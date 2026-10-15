/* ponytail: one owned 26-byte snapshot; RX_STREAM reuses a lossy mailbox.
 * Stream sequence counts accepted SDUs, not radio packets.
 * Install at allocation+576; stock callback ownership stays with the host.
 */
.syntax unified
.cpu cortex-m3
.thumb
.section .text
.global iso_rx_task
.thumb_func
iso_rx_task:
    push.w {r4-r8,lr}
    mov r5,r1
    ldr r7,stock_transport
    blx r7
    mrs r8,APSR
    push.w {r0-r3,r12,lr}
    mrs r0,IPSR
    cbnz r0,task_done
    tst r5,#1
    beq task_done
    adr r6,task_state
.ifndef RX_STREAM
    ldr r0,[r6]
    cbnz r0,task_done
.endif
    adr.w r4,rx_trace
    ldr r0,[r4]
    cmp r0,#2
    bne task_done
    dmb
    ldr r0,[r4,#24]
    ldr r1,expected_header
    cmp r0,r1
    bne task_invalid
    ldr r0,[r4,#44]
    cmp r0,#1
    bne task_invalid
    ldr r0,[r4,#20]
    tst r0,#4
    beq task_invalid
    ldrh r0,[r4,#28]
    adds r0,#1
    uxth r0,r0
    ldrh r1,[r4,#34]
    cmp r0,r1
    bne task_invalid
    ldrh r0,[r4,#30]
    ldrh r1,[r4,#36]
    cmp r0,r1
    bne task_invalid
    ldrh r0,[r4,#32]
    ldrh r1,[r4,#38]
    cmp r0,r1
    bne task_invalid
.ifdef RX_STREAM
    ldr r0,[r6]
    uxth r0,r0
.else
    movs r0,#0
.endif
    add.w r1,r4,#48
    movs r2,#26
    bl iso_rx_emit
    str r0,[r6,#4]
    cmp r0,#0
    bne task_done
.ifdef RX_STREAM
    ldr r0,[r6]
    adds r0,#1
    str r0,[r6]
    b task_rearm
.else
    dmb
    movs r0,#1
    str r0,[r6]
.endif
task_done:
    msr APSR_nzcvq,r8
    pop.w {r0-r3,r12,lr}
    pop.w {r4-r8,pc}
    .balign 4
stock_transport: .word 0x00044105
expected_header: .word 0x1a001a00
    .set rx_trace,iso_rx_task-256
    .space 160-(.-iso_rx_task),0
task_state: .word 0,0
.ifdef RX_STREAM
    .word 0
.thumb_func
task_invalid:
    ldr r0,[r6,#8]
    adds r0,#1
    str r0,[r6,#8]
    movs r0,#2
    str r0,[r6,#4]
task_rearm:
    dmb
    movs r0,#0
    str r0,[r4]
    b task_done
.else
    .set task_invalid,task_done
.endif
    /* Preserve the existing BIS configuration trace at allocation+768. */
    .space 224-(.-iso_rx_task),0
    .include "iso_rx_emit.S"
