#include <stdint.h>
#include <stdio.h>

#include "pico/cyw43_arch.h"
#include "pico/stdlib.h"
#include "cyw43_internal.h"
#include "cyw43_ll.h"

#define BT_BACKPLANE_BASE 0x19000000u
#define BT_ROM_LENGTH 0x00090000u
#define DUMP_CHUNK_SIZE 64u

_Static_assert(DUMP_CHUNK_SIZE <= CYW43_BUS_MAX_BLOCK_SIZE, "dump read exceeds bus buffer");

static uint32_t crc32_update(uint32_t crc, const uint8_t *data, size_t length) {
    while (length-- != 0u) {
        crc ^= *data++;
        for (unsigned bit = 0; bit < 8u; ++bit) {
            crc = (crc >> 1u) ^ (0xedb88320u & (0u - (crc & 1u)));
        }
    }
    return crc;
}

static void print_hex(const uint8_t *data, size_t length) {
    for (size_t index = 0; index < length; ++index) {
        printf("%02x", data[index]);
    }
}

int main(void) {
    static uint8_t buffer[DUMP_CHUNK_SIZE];
    uint32_t image_crc = UINT32_MAX;

    stdio_init_all();
    if (cyw43_arch_init() != 0) {
        printf("CYW43 initialization failed\n");
        return 1;
    }
    if (cyw43_bluetooth_hci_init() != 0) {
        printf("Bluetooth firmware initialization failed\n");
        return 1;
    }

    /* ponytail: repeat dump forever so host can attach mid-flight and still
       capture one complete pass. Ceiling: no flow control. Upgrade: ACK per chunk. */
    for (;;) {
    image_crc = UINT32_MAX;
    printf("CYW43_BTROM_BEGIN %08x %08x\n", 0u, BT_ROM_LENGTH);
    for (uint32_t address = 0; address < BT_ROM_LENGTH; address += DUMP_CHUNK_SIZE) {
        cyw43_arch_lwip_begin();
        int result = cyw43_ll_read_backplane_mem(
            &cyw43_state.cyw43_ll,
            BT_BACKPLANE_BASE + address,
            DUMP_CHUNK_SIZE,
            buffer
        );
        cyw43_arch_lwip_end();
        if (result != 0) {
            printf("CYW43_BTROM_ERROR %08lx %d\n", (unsigned long)address, result);
            return 1;
        }

        uint32_t chunk_crc = ~crc32_update(UINT32_MAX, buffer, sizeof(buffer));
        image_crc = crc32_update(image_crc, buffer, sizeof(buffer));
        printf("@%08lx:", (unsigned long)address);
        print_hex(buffer, sizeof(buffer));
        printf(":%08lx\n", (unsigned long)chunk_crc);
    }
    printf("CYW43_BTROM_END %08lx\n", (unsigned long)~image_crc);
    sleep_ms(500);
  }
}
