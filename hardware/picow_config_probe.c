#include <stdint.h>
#include <stdio.h>

#include "pico/cyw43_arch.h"
#include "pico/stdlib.h"
#include "cyw43_internal.h"
#include "cyw43_ll.h"

#define READ_SIZE 64u
_Static_assert(READ_SIZE <= CYW43_BUS_MAX_BLOCK_SIZE, "read exceeds bus buffer");

static int dump(uint32_t address, uint32_t length) {
    uint8_t bytes[READ_SIZE];
    for (uint32_t offset = 0; offset < length; offset += READ_SIZE) {
        uint32_t count = length - offset;
        if (count > READ_SIZE) count = READ_SIZE;
        cyw43_arch_lwip_begin();
        int result = cyw43_ll_read_backplane_mem(
            &cyw43_state.cyw43_ll, address + offset, count, bytes);
        cyw43_arch_lwip_end();
        if (result != 0) {
            printf("CONFIG_READ_ERROR %08lx %d\n", (unsigned long)(address + offset), result);
            return result;
        }
        printf("@%08lx:", (unsigned long)(address + offset));
        for (uint32_t i = 0; i < count; ++i) printf("%02x", bytes[i]);
        printf("\n");
    }
    return 0;
}

int main(void) {
    stdio_init_all();
    if (cyw43_arch_init() != 0) {
        printf("CYW43 initialization failed\n");
        return 1;
    }
    int startup = cyw43_bluetooth_hci_init();

    // Startup RAM is diagnostic state, never evidence of hook execution.
    for (;;) {
        printf("CONFIG_BEGIN startup=%d\n", startup);
        // ponytail: .0065 RAM layout only; add address options for other ROMs.
        if (dump(0x18000c7c, 4) ||
            dump(0x192004ec, 0x48) ||
            dump(0x19200560, 0xa4) || // Resident arena bounds and four pool descriptors.
            dump(0x19200620, 0x100) ||
            dump(0x19201f28, 17) || // Pool count and four size/count/reserve records.
            dump(0x19202784, 0xe0) ||
            dump(0x192035c0, 16) || // Connection subarena base/end/cursor/override size.
            dump(0x19209bac, 0x80) ||
            dump(0x1921e000, 0x44) ||
            dump(0x1921f000, 0xc80)) return 1;
        printf("CONFIG_END\n");
        sleep_ms(1000);
    }
}
