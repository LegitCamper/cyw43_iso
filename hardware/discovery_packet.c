#include <stdint.h>

/* ponytail: one unencrypted BIS through the stock DTM event; add normal event
 * ownership before sharing the radio with connections or other advertisers. */
void discovery_packet(uint8_t *allocation, uint32_t packet_index) {
#ifdef NATIVE_ISO_QUEUE
    volatile uint32_t *queue = (volatile uint32_t *)(allocation + 1792);
    uint32_t tail = 0;
    int dequeued = 0;
    if (queue[7]) {
        queue[6]++;
        queue[7] = 0;
    }
    __asm__ volatile("dmb sy" ::: "memory");
    if (queue[6] != *(volatile uint32_t *)(allocation + 2092)) {
        uint32_t context = *(volatile uint32_t *)0x201f1c;
        if (context) ((void (*)(uint32_t, uint32_t, uint32_t))0x9061)(context + 0xb4, 1, 0);
    }
#endif
    uint32_t primary_channel = 37;
    uint32_t advertising_reload = 6;
    uint32_t bis_reload = 10;
#ifdef REPEATED_DISCOVERY
    if (packet_index < 12) {
        uint32_t cycle = packet_index / 4;
        uint32_t slot = packet_index % 4;
        if (slot < 3) {
            primary_channel += slot;
            uint32_t offset = (3 - slot) * 125;
            allocation[1031] = offset;
            allocation[1032] = offset >> 8;
            packet_index = 0;
        } else {
            uint32_t offset = 2125 - cycle * 1000;
            allocation[1100] = offset;
            allocation[1101] = offset >> 8;
            advertising_reload = cycle < 2 ? 30 : 6;
            packet_index = 1;
        }
    } else {
#ifdef CONTINUOUS_DISCOVERY
        uint32_t counter = (packet_index - 12) / 5;
        uint32_t slot = (packet_index - 12) % 5;
        if (slot == 2) {
            primary_channel += counter % 3;
            allocation[1031] = 125;
            allocation[1032] = 0;
            packet_index = 0;
        } else if (slot == 3) {
            /* SyncInfo offset describes a 30-us window containing 8750 us. */
            allocation[1100] = 291 & 255;
            allocation[1101] = 291 >> 8;
            uint32_t next_event = 0x1234 + counter + 1;
            allocation[1116] = next_event;
            allocation[1117] = next_event >> 8;
            advertising_reload = 4;
            packet_index = 1;
        } else {
            packet_index = 2 + 4 * counter + (slot == 1 ? 1 : slot == 4 ? 3 : 0);
            bis_reload = slot == 1 ? 6 : 10;
        }
#else
        packet_index -= 10;
#endif
    }
#else
#ifdef MULTICHANNEL_DISCOVERY
    if (packet_index < 3) {
        primary_channel += packet_index;
        uint32_t offset = (3 - packet_index) * 125;
        allocation[1031] = offset;
        allocation[1032] = offset >> 8;
        packet_index = 0;
    } else {
        packet_index -= 2;
    }
#endif
#endif
    volatile uint32_t *control = (volatile uint32_t *)0x318b6c;
    volatile uint32_t *frequency = (volatile uint32_t *)0x3186a4;
    volatile uint16_t *reload = (volatile uint16_t *)0x2097c0;
    uint32_t (*channel)(uint32_t, uint32_t) = (void *)(allocation + 641);
    void (*copy)(uint32_t, uint32_t, const uint8_t *, uint32_t, uint32_t) = (void *)0x44ed3;
    uint32_t address, crc, logical, header;
    const uint8_t *payload;
    if (packet_index < 2) {
        uint8_t *pdu = allocation + (packet_index == 0 ? 1024 : 1088);
        address = 0x8e89bed6;
        crc = 0x555555;
        logical = packet_index == 0 ? primary_channel : 17;
        header = pdu[0] | ((uint32_t)pdu[1] << 8);
        payload = pdu + 2;
        *reload = advertising_reload;
    } else if ((packet_index & 1) == 0) {
        uint32_t counter = (packet_index - 2) / 2;
        uint8_t *pdu = allocation + 1152;
        for (unsigned i = 0; i < 4; ++i) pdu[34 + i] = counter >> (8 * i);
        pdu[38] = 0;
        address = 0x12345678;
        crc = 0xabcdef;
#ifdef CONTINUOUS_DISCOVERY
        logical = channel(0x1234 + counter / 2, 0x444c);
#else
        logical = channel(0x1234 + counter, 0x444c);
#endif
        header = pdu[0] | ((uint32_t)pdu[1] << 8);
        payload = pdu + 2;
        *reload = 6;
    } else {
        uint32_t counter = (packet_index - 3) / 2;
        address = 0x718ebed6;
        crc = 0x123401;
        logical = channel(counter, 0xcf58);
        header = 0x1a00;
        payload = allocation + 320 + 28 * (counter & 3);
#ifdef NATIVE_ISO_QUEUE
        uint32_t head = queue[0];
        tail = queue[1];
        __asm__ volatile("dmb sy" ::: "memory");
        if (head < 5 && tail < 5 && head != tail) {
            payload = allocation + 1824 + 40 * tail;
            dequeued = 1;
        } else {
            header = 1;
            payload = 0;
            queue[5]++;
        }
#endif
        *reload = bis_reload;
    }
    control[1] = address;
    control[14] = crc;
    control[0] |= 0x300000;
    control[34] = logical;
    uint32_t offset = logical == 37 ? 0 : 2 * logical + (logical <= 10 ? 2 : 4);
#ifdef MULTICHANNEL_DISCOVERY
    if (logical == 38) offset = 24;
    if (logical == 39) offset = 78;
#endif
    *frequency = 0x80 | offset;
    copy(0, header, payload, 0, 0);
#ifdef NATIVE_ISO_QUEUE
    if (dequeued) {
        __asm__ volatile("dmb sy" ::: "memory");
        queue[1] = tail == 4 ? 0 : tail + 1;
        queue[4]++;
        queue[7] = 1;
    }
#endif
}
