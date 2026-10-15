use std::env;
use std::error::Error;
use std::fmt::Write as _;
use std::fs;
use std::io;
use std::path::Path;

use cyw43_iso_re::{Firmware, RecordKind, hcd_to_firmware};

fn main() {
    if let Err(error) = run() {
        eprintln!("error: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn Error>> {
    let mut args = env::args().skip(1);
    let Some(command) = args.next() else {
        return Err(
            "usage: cyw43-iso-re <inspect|patch-map|instrument-hook|c-header|verify|extract|decode-dump|import-hcd> <input> [output] [argument]".into(),
        );
    };
    let Some(input) = args.next() else {
        return Err("missing input path".into());
    };

    match command.as_str() {
        "inspect" => {
            let bytes = fs::read(&input)?;
            inspect(&Firmware::parse(&bytes)?);
        }
        "patch-map" => {
            let bytes = fs::read(&input)?;
            patch_map(&Firmware::parse(&bytes)?);
        }
        "instrument-hook" => {
            let Some(output) = args.next() else {
                return Err("missing output file".into());
            };
            let Some(rom_pointer) = args.next() else {
                return Err("missing ROM Thumb pointer".into());
            };
            let bytes = fs::read(&input)?;
            let image = Firmware::parse(&bytes)?.with_hook_counter(parse_u32(&rom_pointer)?)?;
            fs::write(output, image.bytes())?;
            println!(
                "wrapper=0x{:08x} counter=0x{:08x} original=0x{:08x}",
                image.wrapper_address(),
                image.counter_address(),
                image.original_patch_pointer()
            );
        }
        "c-header" => {
            let Some(output) = args.next() else {
                return Err("missing output file".into());
            };
            let bytes = fs::read(&input)?;
            let firmware = Firmware::parse(&bytes)?;
            fs::write(output, c_header(&firmware, &bytes))?;
        }
        "verify" => {
            let bytes = fs::read(&input)?;
            verify(&Firmware::parse(&bytes)?, &bytes)?;
        }
        "extract" => {
            let Some(output) = args.next() else {
                return Err("missing output directory".into());
            };
            let bytes = fs::read(&input)?;
            extract(&Firmware::parse(&bytes)?, Path::new(&output))?;
        }
        "decode-dump" => {
            let Some(output) = args.next() else {
                return Err("missing output file".into());
            };
            decode_dump(&fs::read_to_string(input)?, Path::new(&output))?;
        }
        "import-hcd" => {
            let Some(output) = args.next() else {
                return Err("missing output file".into());
            };
            let Some(version) = args.next() else {
                return Err("missing firmware version".into());
            };
            let converted = hcd_to_firmware(&version, &fs::read(input)?)?;
            fs::write(output, converted)?;
        }
        _ => return Err(format!("unknown command: {command}").into()),
    }

    if let Some(extra) = args.next() {
        return Err(format!("unexpected argument: {extra}").into());
    }
    Ok(())
}

fn parse_u32(value: &str) -> Result<u32, std::num::ParseIntError> {
    u32::from_str_radix(value.strip_prefix("0x").unwrap_or(value), 16)
}

fn c_header(firmware: &Firmware<'_>, bytes: &[u8]) -> String {
    let mut output = format!("// BT FW download, version = {}\n", firmware.version());
    output.push_str("const unsigned char cyw43_btfw_43439[] CYW43_RESOURCE_ATTRIBUTE = {\n");
    for chunk in bytes.chunks(12) {
        output.push_str("  ");
        for byte in chunk {
            write!(output, "0x{byte:02x}, ").expect("writing to a String cannot fail");
        }
        output.push('\n');
    }
    output.push_str("};\n");
    writeln!(
        output,
        "const unsigned int cyw43_btfw_43439_len = {};",
        bytes.len()
    )
    .expect("writing to a String cannot fail");
    output.push_str("const uint8_t *btfw_data = &cyw43_btfw_43439[0];\n");
    output.push_str("const size_t btfw_len = cyw43_btfw_43439_len;\n");
    output
}

fn inspect(firmware: &Firmware<'_>) {
    println!("version: {}", firmware.version());
    println!("records: {}", firmware.records().len());
    for (index, segment) in firmware.segments().iter().enumerate() {
        println!(
            "segment {index}: 0x{:08x}..0x{:08x} ({} bytes)",
            segment.address(),
            segment.end_address(),
            segment.len()
        );
    }
    for record in firmware.records() {
        if let RecordKind::Unknown { type_id, data } = record.kind() {
            println!("unknown record type {type_id} ({} bytes)", data.len());
        }
    }
}

fn patch_map(firmware: &Firmware<'_>) {
    let hooks = firmware.patch_hooks();
    println!("hooks: {}", hooks.len());
    for hook in hooks {
        println!(
            "ROM 0x{:08x} -> Patchram 0x{:08x} (kind {})",
            hook.rom_pointer(),
            hook.patch_pointer(),
            hook.kind()
        );
    }
}

fn verify(firmware: &Firmware<'_>, original: &[u8]) -> Result<(), Box<dyn Error>> {
    if firmware.encode()? != original {
        return Err("rebuilt blob differs from input".into());
    }
    println!("exact round trip verified");
    Ok(())
}

fn extract(firmware: &Firmware<'_>, output: &Path) -> Result<(), io::Error> {
    fs::create_dir_all(output)?;
    let mut manifest = format!("version={}\n", firmware.version());

    // ponytail: sparse regions stay raw; emit ELF after controller ISA and ROM map are hardware-verified.
    for segment in firmware.segments() {
        let name = format!(
            "segment_{:08x}_{:08x}.bin",
            segment.address(),
            segment.end_address()
        );
        fs::write(output.join(&name), segment.to_vec())?;
        writeln!(
            manifest,
            "{name} address=0x{:08x} length={}",
            segment.address(),
            segment.len()
        )
        .expect("writing to a String cannot fail");
    }
    fs::write(output.join("manifest.txt"), manifest)
}

fn decode_dump(capture: &str, output: &Path) -> Result<(), Box<dyn Error>> {
    const MAX_BACKPLANE_DUMP_CHUNK: usize = 64;

    let mut image = Vec::new();
    let mut expected_address = None;
    let mut expected_length = None;
    let mut completed_crc = None;
    let mut dump_kind = None;

    for line in capture.lines() {
        let header = line
            .strip_prefix("CYW43_BTROM_BEGIN ")
            .map(|header| ("BTROM", header))
            .or_else(|| {
                line.strip_prefix("CYW43_BTRAM_BEGIN ")
                    .map(|header| ("BTRAM", header))
            });
        if let Some((kind, header)) = header {
            let fields: Vec<_> = header.split_ascii_whitespace().collect();
            if fields.len() != 2 {
                return Err("invalid dump header".into());
            }
            image.clear();
            completed_crc = None;
            dump_kind = Some(kind);
            expected_address = Some(u32::from_str_radix(fields[0], 16)?);
            expected_length = Some(usize::from_str_radix(fields[1], 16)?);
        } else if let Some(record) = line.strip_prefix('@') {
            if dump_kind.is_none() {
                continue;
            }
            let fields: Vec<_> = record.split(':').collect();
            if fields.len() != 3 {
                return Err("invalid dump record".into());
            }
            let address = u32::from_str_radix(fields[0], 16)?;
            if Some(address) != expected_address {
                return Err(format!("unexpected dump address {address:#010x}").into());
            }
            let data = decode_hex(fields[1])?;
            if data.len() > MAX_BACKPLANE_DUMP_CHUNK {
                return Err(format!(
                    "dump chunk {} exceeds {MAX_BACKPLANE_DUMP_CHUNK}-byte SPI limit",
                    data.len()
                )
                .into());
            }
            let expected_crc = u32::from_str_radix(fields[2], 16)?;
            if crc32(&data) != expected_crc {
                return Err(format!("CRC mismatch at {address:#010x}").into());
            }
            expected_address = address.checked_add(u32::try_from(data.len())?);
            image.extend_from_slice(&data);
        } else {
            let footer = line
                .strip_prefix("CYW43_BTROM_END ")
                .map(|crc| ("BTROM", crc))
                .or_else(|| {
                    line.strip_prefix("CYW43_BTRAM_END ")
                        .map(|crc| ("BTRAM", crc))
                });
            if let Some((kind, crc)) = footer {
                if dump_kind.is_none() {
                    continue;
                }
                if dump_kind != Some(kind) {
                    return Err("dump header and footer types differ".into());
                }
                completed_crc = Some(u32::from_str_radix(crc, 16)?);
                break;
            }
        }
    }

    let expected_length = expected_length.ok_or("dump header not found")?;
    if image.len() != expected_length {
        return Err(format!("dump length {} != expected {expected_length}", image.len()).into());
    }
    let expected_crc = completed_crc.ok_or("dump end marker not found")?;
    if crc32(&image) != expected_crc {
        return Err("full image CRC mismatch".into());
    }
    fs::write(output, &image)?;
    println!("decoded {} bytes, CRC32 {expected_crc:08x}", image.len());
    Ok(())
}

fn decode_hex(input: &str) -> Result<Vec<u8>, Box<dyn Error>> {
    let (pairs, remainder) = input.as_bytes().as_chunks::<2>();
    if !remainder.is_empty() {
        return Err("hex payload has odd length".into());
    }
    pairs
        .iter()
        .map(|pair| Ok(u8::from_str_radix(std::str::from_utf8(pair)?, 16)?))
        .collect()
}

fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = u32::MAX;
    for &byte in bytes {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xedb8_8320 & 0_u32.wrapping_sub(crc & 1));
        }
    }
    !crc
}

#[cfg(test)]
mod tests {
    use super::{crc32, decode_dump};
    use std::fs;

    #[test]
    fn crc32_should_match_standard_check_value() {
        assert_eq!(crc32(b"123456789"), 0xcbf4_3926);
    }

    #[test]
    fn decode_dump_should_rebuild_valid_capture() {
        let output =
            std::env::temp_dir().join(format!("cyw43-rom-dump-{}.bin", std::process::id()));
        let capture = "@00000100:00:d202ef8d\nCYW43_BTROM_END 00000000\nCYW43_BTROM_BEGIN 00000000 00000004\n@00000000:01020304:b63cfbcd\nCYW43_BTROM_END b63cfbcd\n";

        decode_dump(capture, &output).expect("capture must decode");
        let decoded = fs::read(&output).expect("decoded image must exist");
        fs::remove_file(output).expect("temporary image must be removable");

        assert_eq!(decoded, [1, 2, 3, 4]);
    }

    #[test]
    fn decode_dump_should_support_ram_capture() {
        let output =
            std::env::temp_dir().join(format!("cyw43-ram-dump-{}.bin", std::process::id()));
        let capture = "CYW43_BTRAM_BEGIN 00000000 00000004\n@00000000:01020304:b63cfbcd\nCYW43_BTRAM_END b63cfbcd\n";

        decode_dump(capture, &output).expect("RAM capture must decode");
        let decoded = fs::read(&output).expect("decoded image must exist");
        fs::remove_file(output).expect("temporary image must be removable");

        assert_eq!(decoded, [1, 2, 3, 4]);
    }

    #[test]
    fn decode_dump_should_reject_oversized_spi_chunk() {
        let output =
            std::env::temp_dir().join(format!("cyw43-bad-dump-{}.bin", std::process::id()));
        let payload = "00".repeat(65);
        let capture = format!(
            "CYW43_BTROM_BEGIN 00000000 00000041\n@00000000:{payload}:{:08x}\nCYW43_BTROM_END {:08x}\n",
            crc32(&[0; 65]),
            crc32(&[0; 65])
        );

        let error = decode_dump(&capture, &output).expect_err("oversized chunk must fail");
        assert!(error.to_string().contains("exceeds 64-byte SPI limit"));
        assert!(!output.exists());
    }
}
