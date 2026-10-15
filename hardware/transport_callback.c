#include <stdint.h>
#include <stddef.h>

/* ponytail: one 26-byte, complete, timestamp-free SDU on one diagnostic handle;
 * add negotiated handles and fragments with controller procedures. */
struct ingress_state {
    uint32_t allocation, handle, blocked, accepted, callbacks, stock_packets;
    uint32_t last_sequence, inspections, credit_emitter, reported;
};

enum head_kind { EMPTY, NORMAL, BLOCKED };

static uint32_t mapped(uint32_t address) {
    return ((uint32_t (*)(uint32_t))0x7baed)(address);
}

static void barrier(void) { __asm__ volatile("dmb sy" ::: "memory"); }

static uint8_t ring_byte(volatile uint8_t *ring, uint32_t offset) {
    return ring[offset & 4095];
}

static uint32_t ring_u16(volatile uint8_t *ring, uint32_t offset) {
    return ring_byte(ring, offset) | ((uint32_t)ring_byte(ring, offset + 1) << 8);
}

static void service_credits(struct ingress_state *state) {
    if (!state->allocation || !state->credit_emitter || state->handle != 0x42) return;
    volatile uint32_t *queue = (volatile uint32_t *)(state->allocation + 1792);
    uint32_t completed = queue[6];
    barrier();
    uint32_t count = completed - state->reported;
    if (count && count <= 4 && ((uint32_t (*)(uint32_t))state->credit_emitter)(count))
        state->reported = completed;
}

/* Only this transport task changes OUT. IRQ arrival hooks retain stock behavior. */
static enum head_kind inspect_head(struct ingress_state *state) {
    volatile uint32_t *global = (volatile uint32_t *)0x2038cc;
    ((void (*)(void))0x5f941)();
    state->inspections++;
    state->blocked = 4;
    if (global[4] != 4096 || global[5] != 4096 || !global[3]
        || state->allocation == 0 || state->handle > 0xeff) return BLOCKED;
    uint32_t base = global[3];
    uint32_t start = mapped(base);
    if (mapped(base + 4095) != start + 4095) return BLOCKED;
    volatile uint8_t *ring = (volatile uint8_t *)start;
    volatile uint32_t *producer = (volatile uint32_t *)mapped(base + 8192);
    volatile uint32_t *consumer = (volatile uint32_t *)mapped(base + 8196);
    volatile uint32_t *queue = (volatile uint32_t *)(state->allocation + 1792);
    for (unsigned budget = 0; budget < 5; ++budget) {
        uint32_t out = *consumer, in = *producer;
        barrier();
        if (out >= 4096 || in >= 4096 || ((out | in) & 3)) return BLOCKED;
        uint32_t available = (in - out) & 4095;
        if (available == 0) { state->blocked = 0; return EMPTY; }
        if (available < 4) { state->blocked = 3; return BLOCKED; }
        uint32_t length = ring_u16(ring, out) | ((uint32_t)ring_byte(ring, out + 2) << 16);
        if (length > 4088) return BLOCKED;
        uint32_t occupied = (length + 7) & ~3u;
        if (available < occupied) { state->blocked = 3; return BLOCKED; }
        if (ring_byte(ring, out + 3) != 5) { state->blocked = 0; return NORMAL; }
        state->blocked = 2;
        if (length != 34 || ring_u16(ring, out + 4) != (state->handle | 0x2000)
            || ring_u16(ring, out + 6) != 30 || ring_u16(ring, out + 10) != 26)
            return BLOCKED;
        state->blocked = 4;
        uint32_t head = queue[0], tail = queue[1];
        if (head > 4 || tail > 4) return BLOCKED;
        uint32_t next = head == 4 ? 0 : head + 1;
        if (next == tail || budget == 4) { state->blocked = 1; return BLOCKED; }
        volatile uint8_t *slot = (volatile uint8_t *)(state->allocation + 1824 + head * 40);
        for (unsigned i = 0; i < 26; ++i) slot[i] = ring_byte(ring, out + 12 + i);
        state->last_sequence = ring_u16(ring, out + 8);
        barrier();
        queue[0] = next;
        queue[2]++;
        state->accepted++;
        barrier();
        *consumer = (out + occupied) & 4095;
        if (global[1] & 1) ((void (*)(void))0x5f87b)();
    }
    return BLOCKED;
}

static void process_input(uint8_t *context, struct ingress_state *state) {
    if (inspect_head(state) == NORMAL) {
        void *packet = ((void *(*)(void *))0x5fa0f)(context);
        state->stock_packets++;
        if (packet) ((void (*)(void *, void *))0x1490f)(context, packet);
    }
}

/* Faithful stock 0x44104 message dispatch, with ISO inspection at each parser call. */
void native_transport(uint8_t *context, uint32_t events, struct ingress_state *state) {
    if (!(events & 1)) return;
    state->callbacks++;
    service_credits(state);
    /* RF completion can set task event bit 0 without allocating a queue message. */
    if (state->blocked == 1) process_input(context, state);
    uint32_t *message;
    while ((message = ((uint32_t *(*)(void *))0x17ed5)(context + 0xd8)) != NULL) {
        uint32_t kind = message[1];
        switch (kind) {
        case 0x7041:
        case 0x7043:
        case 0x7084:
            ((void (*)(void *, void *))0x440ab)(context, message);
            continue; /* Outgoing helper owns this message, as in stock callback. */
        case 0xc000:
        case 0xc001:
            if (kind == 0xc001) ((void (*)(uint32_t))0x44025)(1);
            process_input(context, state);
            ((void (*)(uint32_t))0x44025)(0);
            if (((uint32_t (*)(void *))0x440c3)(context) && kind == 0xc000)
                ((void (*)(void *))0x4406b)(context);
            break;
        case 0xc002:
            ((void (*)(uint32_t))0x418bf)(0);
            /* Stock deliberately falls through to its wake handler. */
            __attribute__((fallthrough));
        case 0xc003:
            ((void (*)(void))0x440dd)();
            break;
        case 0xc004:
            ((void (*)(void))0x440e3)();
            break;
        default:
            break;
        }
        ((void (*)(void *))0x17adb)(message);
    }
    service_credits(state);
}
