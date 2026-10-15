/* ponytail: task-only, fixed handle 0x42 and complete 26-byte valid SDUs.
 * Add negotiated streams/status/timestamps when controller RX supports them.
 * emit(sequence, SRAM payload, length): 0 copied, 1 ring full, 2 invalid.
 * Caller owns input throughout; retry only result 1. Never call from ISR.
 */
.syntax unified
.cpu cortex-m3
.thumb
.section .text
.global iso_rx_emit
.thumb_func
iso_rx_emit:
    mrs r3,IPSR
    cbnz r3,invalid
    cmp r2,#26
    bne invalid
    lsrs r3,r0,#16
    bne invalid
    sub.w r3,r1,#0x200000
    ldr r12,last_payload_offset
    cmp r3,r12
    bhi invalid
    push {r4,lr}
    sub sp,#40
    movw r3,#0x2042
    strh r3,[sp]
    movs r3,#30
    strh r3,[sp,#2]
    strh r0,[sp,#4]
    strh r2,[sp,#6]
    add r3,sp,#8
copy:
    ldrb r0,[r1],#1
    strb r0,[r3],#1
    subs r2,#1
    bne copy
    movs r0,#5
    mov r1,sp
    movs r2,#34
    ldr r3,ring_write
    blx r3
    mov r4,r0
    cbnz r0,done
    dmb
    ldr r3,host_notify
    blx r3
done:
    mov r0,r4
    add sp,#40
    pop {r4,pc}
invalid:
    movs r0,#2
    bx lr
    .balign 4
last_payload_offset: .word 0x1ffe6
ring_write: .word 0x0005fc55
host_notify: .word 0x0005f87b
