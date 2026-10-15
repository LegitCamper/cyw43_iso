/* RAM transport callback. Install at context+0x100 only after filling state.
 * State+0 is the existing resident allocation; +4 is diagnostic ISO handle.
 * Both producers must run in the single transport task, never an ISR.
 */
.syntax unified
.cpu cortex-m3
.thumb
.section .text
.global transport_callback, transport_state
.thumb_func
transport_callback:
    adr r2,transport_state
    b.w native_transport
    .balign 4
transport_state:
    /* allocation, handle, blocked, accepted, callbacks, stock_packets,
     * last_sequence, inspections, credit_emitter, reported. blocked: 0 none, 1 full, 2 unsupported,
     * 3 incomplete frame, 4 corrupt ring/queue state. */
    .word 0,0x0042,0,0,0,0,0,0,0,0
