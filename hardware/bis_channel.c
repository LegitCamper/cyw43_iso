/* Core Vol 6 Part B 4.5.8.3: first-subevent CSA #2, no MMIO or global state. */
#include <stdint.h>

/* ponytail: all 37 channels, NSE=1; add remapping/subevents before other BIGs. */
uint32_t bis_channel(uint32_t event_counter, uint32_t channel_id) {
    channel_id &= 0xffff;
    uint32_t value = (event_counter ^ channel_id) & 0xffff;
    for (unsigned round = 0; round < 3; ++round) {
        value = ((value & 0xaaaa) >> 1) | ((value & 0x5555) << 1);
        value = ((value & 0xcccc) >> 2) | ((value & 0x3333) << 2);
        value = ((value & 0xf0f0) >> 4) | ((value & 0x0f0f) << 4);
        value = (17 * value + channel_id) & 0xffff;
    }
    return (value ^ channel_id) % 37;
}
