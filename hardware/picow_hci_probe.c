#include <stdbool.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>

#include "btstack.h"
#include "pico/btstack_cyw43.h"
#include "pico/cyw43_arch.h"
#include "pico/stdlib.h"

enum probe_state {
    PROBE_IDLE,
    PROBE_VERSION,
    PROBE_COMMANDS,
    PROBE_FEATURES,
    PROBE_DONE,
};

static enum probe_state state;
static btstack_packet_callback_registration_t event_callback;
static uint16_t raw_opcode;
static unsigned raw_index;
static const uint16_t raw_iso_opcodes[] = {
    0x2060, 0x2061, 0x2062, 0x2063, 0x2064, 0x2065, 0x2066, 0x2067,
    0x2068, 0x2069, 0x206a, 0x206b, 0x206c, 0x206d, 0x206e, 0x206f,
    0x2070, 0x2071, 0x2072, 0x2073, 0x2074, 0x2075, 0x2076,
};

static void send_raw_iso_command(void) {
    raw_opcode = raw_iso_opcodes[raw_index++];
    /* CYW43 transport writes a four-byte prefix and reads whole words. */
    uint8_t wire[8] __attribute__((aligned(4))) = {0};
    wire[4] = (uint8_t)raw_opcode;
    wire[5] = (uint8_t)(raw_opcode >> 8);
    hci_send_cmd_packet(wire + 4, 3);
}

static bool bitmap_test(const volatile uint8_t *bitmap, unsigned bit) {
    return (bitmap[bit / 8] & (1u << (bit % 8))) != 0;
}

static void self_check(void) {
    volatile uint8_t bitmap[64] = {0};
    bitmap[41] = 1u << 7;
    if (!bitmap_test(bitmap, 41 * 8 + 7) || bitmap_test(bitmap, 42 * 8)) {
        panic("bitmap self-check failed");
    }
}

static void print_flag(const char *name, bool supported) {
    printf("  %-31s %s\n", name, supported ? "yes" : "no");
}

static void print_hex(const uint8_t *data, size_t length) {
    for (size_t i = 0; i < length; ++i) {
        printf("%02x", data[i]);
    }
    printf("\n");
}

static void print_iso_commands(const uint8_t *commands) {
    printf("ISO HCI commands:\n");
    print_flag("LE Read Buffer Size v2", bitmap_test(commands, 41 * 8 + 5));
    print_flag("LE Read ISO TX Sync", bitmap_test(commands, 41 * 8 + 6));
    print_flag("LE Set CIG Parameters", bitmap_test(commands, 41 * 8 + 7));
    print_flag("LE Set CIG Parameters Test", bitmap_test(commands, 42 * 8 + 0));
    print_flag("LE Create CIS", bitmap_test(commands, 42 * 8 + 1));
    print_flag("LE Remove CIG", bitmap_test(commands, 42 * 8 + 2));
    print_flag("LE Accept CIS Request", bitmap_test(commands, 42 * 8 + 3));
    print_flag("LE Reject CIS Request", bitmap_test(commands, 42 * 8 + 4));
    print_flag("LE Create BIG", bitmap_test(commands, 42 * 8 + 5));
    print_flag("LE Create BIG Test", bitmap_test(commands, 42 * 8 + 6));
    print_flag("LE Terminate BIG", bitmap_test(commands, 42 * 8 + 7));
    print_flag("LE BIG Create Sync", bitmap_test(commands, 43 * 8 + 0));
    print_flag("LE BIG Terminate Sync", bitmap_test(commands, 43 * 8 + 1));
    print_flag("LE Setup ISO Data Path", bitmap_test(commands, 43 * 8 + 3));
    print_flag("LE Remove ISO Data Path", bitmap_test(commands, 43 * 8 + 4));
    print_flag("LE ISO Transmit Test", bitmap_test(commands, 43 * 8 + 5));
    print_flag("LE ISO Receive Test", bitmap_test(commands, 43 * 8 + 6));
    print_flag("LE ISO Read Test Counters", bitmap_test(commands, 43 * 8 + 7));
    print_flag("LE ISO Test End", bitmap_test(commands, 44 * 8 + 0));
    print_flag("LE Read ISO Link Quality", bitmap_test(commands, 44 * 8 + 2));
}

static void print_iso_features(const uint8_t *features) {
    bool cis_central = bitmap_test(features, 28);
    bool cis_peripheral = bitmap_test(features, 29);
    bool iso_broadcaster = bitmap_test(features, 30);
    bool sync_receiver = bitmap_test(features, 31);
    bool iso_channels = bitmap_test(features, 32);

    printf("LE ISO features:\n");
    print_flag("CIS Central", cis_central);
    print_flag("CIS Peripheral", cis_peripheral);
    print_flag("ISO Broadcaster", iso_broadcaster);
    print_flag("Synchronized Receiver", sync_receiver);
    print_flag("Isochronous Channels (Host)", iso_channels);
    printf("\nRESULT: controller ISO support %s\n",
           (cis_central || cis_peripheral || iso_broadcaster || sync_receiver) ? "present" : "absent");
}

static void send_next_command(void) {
    switch (state) {
        case PROBE_VERSION:
            hci_send_cmd(&hci_read_local_version_information);
            break;
        case PROBE_COMMANDS:
            hci_send_cmd(&hci_read_local_supported_commands);
            break;
        case PROBE_FEATURES:
            hci_send_cmd(&hci_le_read_local_supported_features);
            break;
        default:
            break;
    }
}

static void packet_handler(uint8_t packet_type, uint16_t channel, uint8_t *packet, uint16_t size) {
    (void)channel;

    if (packet_type != HCI_EVENT_PACKET) return;

    if (hci_event_packet_get_type(packet) == BTSTACK_EVENT_STATE) {
        if (btstack_event_state_get_state(packet) != HCI_STATE_WORKING || state != PROBE_IDLE) return;
        printf("CYW43439 Bluetooth ISO capability probe\n\n");
        state = PROBE_VERSION;
        send_next_command();
        return;
    }

    if (hci_event_packet_get_type(packet) != HCI_EVENT_COMMAND_COMPLETE || size < 6) return;

    uint16_t opcode = hci_event_command_complete_get_command_opcode(packet);
    if (state == PROBE_DONE && opcode == raw_opcode) {
        printf("  opcode 0x%04x status 0x%02x size %u\n", opcode, packet[5], size);
        if (raw_index < sizeof(raw_iso_opcodes) / sizeof(raw_iso_opcodes[0])) {
            send_raw_iso_command();
        } else {
            fflush(stdout);
            sleep_ms(1000);
            printf("CYW43_ISO_PROBE_REPEAT\n");
            state = PROBE_VERSION;
            send_next_command();
        }
        return;
    }
    bool expected =
        (state == PROBE_VERSION && opcode == HCI_OPCODE_HCI_READ_LOCAL_VERSION_INFORMATION) ||
        (state == PROBE_COMMANDS && opcode == HCI_OPCODE_HCI_READ_LOCAL_SUPPORTED_COMMANDS) ||
        (state == PROBE_FEATURES && opcode == HCI_OPCODE_HCI_LE_READ_LOCAL_SUPPORTED_FEATURES);
    if (!expected) return;

    uint8_t status = packet[5];
    if (status != ERROR_CODE_SUCCESS) {
        printf("Command 0x%04x failed: status 0x%02x\n", opcode, status);
        state = PROBE_DONE;
        return;
    }

    if (state == PROBE_VERSION && opcode == HCI_OPCODE_HCI_READ_LOCAL_VERSION_INFORMATION) {
        if (size < 14) return;
        printf("Controller version:\n");
        printf("  HCI version                     0x%02x\n", packet[6]);
        printf("  HCI revision                    0x%04x\n", little_endian_read_16(packet, 7));
        printf("  LMP version                     0x%02x\n", packet[9]);
        printf("  manufacturer                    0x%04x\n", little_endian_read_16(packet, 10));
        printf("  LMP subversion                  0x%04x\n\n", little_endian_read_16(packet, 12));
        state = PROBE_COMMANDS;
        send_next_command();
        return;
    }

    if (state == PROBE_COMMANDS && opcode == HCI_OPCODE_HCI_READ_LOCAL_SUPPORTED_COMMANDS) {
        if (size < 70) return;
        printf("Supported command bitmap: ");
        print_hex(&packet[6], 64);
        print_iso_commands(&packet[6]);
        printf("\n");
        state = PROBE_FEATURES;
        send_next_command();
        return;
    }

    if (state == PROBE_FEATURES && opcode == HCI_OPCODE_HCI_LE_READ_LOCAL_SUPPORTED_FEATURES) {
        if (size < 14) return;
        printf("LE feature bitmap: ");
        print_hex(&packet[6], 8);
        print_iso_features(&packet[6]);
        printf("\nRaw zero-parameter ISO opcode probe:\n");
        fflush(stdout);
        state = PROBE_DONE;
        raw_index = 0;
        send_raw_iso_command();
    }
}

int main(void) {
    self_check();
    stdio_init_all();

    if (cyw43_arch_init() != 0) {
        printf("CYW43 initialization failed\n");
        return 1;
    }

    event_callback.callback = packet_handler;
    hci_add_event_handler(&event_callback);
    hci_power_control(HCI_POWER_ON);

    while (true) {
        async_context_poll(cyw43_arch_async_context());
        async_context_wait_for_work_until(cyw43_arch_async_context(), at_the_end_of_time);
    }
}
