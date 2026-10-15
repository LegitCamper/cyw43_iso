#include <stdint.h>
#include <stdio.h>
#include <string.h>

#include "btstack.h"
#include "pico/btstack_cyw43.h"
#include "pico/cyw43_arch.h"
#include "pico/stdlib.h"
#include "cyw43_internal.h"
#include "cyw43_ll.h"

#define BT_BACKPLANE_BASE 0x19000000u

struct hook_trace {
    uint32_t count;
    uint32_t registers[4];
};

static btstack_packet_callback_registration_t event_callback;
static const uint8_t adv_data[] = {
    0x02, BLUETOOTH_DATA_TYPE_FLAGS, 0x06,
    0x0f, BLUETOOTH_DATA_TYPE_COMPLETE_LOCAL_NAME,
    'I', 'S', 'O', ' ', 'H', 'o', 'o', 'k', ' ', 'P', 'r', 'o', 'b', 'e',
};

static void packet_handler(uint8_t packet_type, uint16_t channel, uint8_t *packet, uint16_t size) {
    (void)channel;
    (void)size;
    if (packet_type != HCI_EVENT_PACKET) return;
    if (hci_event_packet_get_type(packet) != BTSTACK_EVENT_STATE) return;
    if (btstack_event_state_get_state(packet) != HCI_STATE_WORKING) return;

    bd_addr_t null_addr = {0};
    gap_advertisements_set_params(160, 160, 0, 0, null_addr, 0x07, 0x00);
    gap_advertisements_set_data(sizeof(adv_data), (uint8_t *)adv_data);
    gap_advertisements_enable(1);
    printf("CYW43_HOOK_COUNTER advertising\n");
}

int main(void) {
    uint32_t previous = UINT32_MAX;

    stdio_init_all();
    if (cyw43_arch_init() != 0) {
        printf("CYW43 initialization failed\n");
        return 1;
    }

    event_callback.callback = packet_handler;
    hci_add_event_handler(&event_callback);
    int startup_result = hci_power_control(HCI_POWER_ON);
    if (startup_result != 0) {
        printf("CYW43_HOOK_COUNTER_INIT_ERROR %d\n", startup_result);
        return 1;
    }
    printf("CYW43_HOOK_COUNTER address=%08lx\n", (unsigned long)HOOK_COUNTER_ADDRESS);

    for (;;) {
        struct hook_trace trace = {0};
        async_context_poll(cyw43_arch_async_context());
        if (hci_get_state() != HCI_STATE_WORKING) {
            async_context_wait_for_work_until(cyw43_arch_async_context(), make_timeout_time_ms(10));
            continue;
        }

        cyw43_arch_lwip_begin();
        int result = cyw43_ll_read_backplane_mem(
            &cyw43_state.cyw43_ll,
            BT_BACKPLANE_BASE + HOOK_COUNTER_ADDRESS,
            sizeof(trace),
            (uint8_t *)&trace
        );
        cyw43_arch_lwip_end();
        if (result != 0) {
            printf("CYW43_HOOK_COUNTER_ERROR %d\n", result);
            return 1;
        }
        if (trace.count != previous) {
            printf(
                "CYW43_HOOK_COUNTER value=%08lx r0=%08lx r1=%08lx r2=%08lx r3=%08lx\n",
                (unsigned long)trace.count,
                (unsigned long)trace.registers[0],
                (unsigned long)trace.registers[1],
                (unsigned long)trace.registers[2],
                (unsigned long)trace.registers[3]
            );
            previous = trace.count;
        }
        async_context_wait_for_work_until(cyw43_arch_async_context(), make_timeout_time_ms(10));
    }
}
