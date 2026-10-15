.syntax unified
.cpu cortex-m3
.thumb
.section .text
.global hook
.thumb_func
hook:
    push.w {r0-r9,r12,lr}
    mrs r8,APSR
    ldr r4,event_address
    cmp r0,r4
    bne passthrough
    mov r6,r0
    ldr r5,[r0,#28]
    ldr r4,parameter_address
    cmp r5,r4
    bne passthrough
    ldrb r0,[r5,#6]
    cmp r0,#1
    bne passthrough
    ldr r7,rx_status
    ldr r7,[r7]
    tst r7,#4
.ifdef RX_STREAM
    beq.w stream_forward
    ldr r0,rx_header
    ldr r0,[r0]
    ldr r1,stream_header
    cmp r0,r1
    bne.w stream_forward
.else
    beq passthrough
.endif
    adr r4,trace
    mrs r9,PRIMASK
    cpsid i
    ldr r0,[r4]
    cmp r0,#0
    bne already_claimed
    movs r0,#1
    str r0,[r4]
    msr PRIMASK,r9
    mrs r0,IPSR
    str r0,[r4,#8]
    str r6,[r4,#12]
    str r5,[r4,#16]
    str r7,[r4,#20]
    ldr r0,rx_header
    ldr r0,[r0]
    str r0,[r4,#24]
    ldrh r0,[r5,#10]
    strh r0,[r4,#28]
    ldrh r0,[r5,#12]
    strh r0,[r4,#30]
    ldrh r0,[r5,#14]
    strh r0,[r4,#32]
    ldrb r0,[r5,#6]
    str r0,[r4,#44]
    ldr r0,rx_window
    add.w r1,r4,#48
    movs r2,#8
copy_window:
    ldr r3,[r0],#4
    str r3,[r1],#4
    subs r2,#1
    bne copy_window
    ldmia.w sp,{r0-r3}
    ldr r12,[sp,#40]
    msr APSR_nzcvq,r8
    ldr r7,stock_rx
    blx r7
    push.w {r0-r3,r12,lr}
    mrs r8,APSR
    ldrh r0,[r5,#10]
    strh r0,[r4,#34]
    ldrh r0,[r5,#12]
    strh r0,[r4,#36]
    ldrh r0,[r5,#14]
    strh r0,[r4,#38]
    ldrsb r0,[r6,#44]
    str r0,[r4,#40]
    dmb
    movs r0,#2
    str r0,[r4]
.ifdef RX_STREAM
    bl stream_wake
.endif
rx_return:
    msr APSR_nzcvq,r8
    pop.w {r0-r3,r12,lr}
    add sp,#16
    pop.w {r4-r9}
    add sp,#4
    pop {pc}
already_claimed:
    msr PRIMASK,r9
.ifdef RX_STREAM
    b.w stream_forward
.endif
passthrough:
    msr APSR_nzcvq,r8
    pop.w {r0-r9,r12,lr}
    ldr.w pc,stock_rx
    .balign 4
event_address: .word 0x00209788
parameter_address: .word 0x002097b8
rx_status: .word 0x00318bac
rx_header: .word 0x00318b98
rx_window: .word 0x00370a00
stock_rx: .word 0x0007143f
.ifdef RX_STREAM
stream_header: .word 0x1a001a00
.endif
    .space 256-(.-hook),0
callbacks:
    .word 0x712f3,0x7131f,0x71321,0x71345,0x71547,0x713e3
    .word 0x7143f,0x714cf,0,0x713fd,0x7154f
    .space 316-(.-hook),0
guard_before: .word 0x52585331
trace:
    .word 0,1
    .space 72
guard_after: .word 0x52585331
.ifdef BIS_PACKET
    .space 416-(.-hook),0
.thumb_func
bis_phase:
    push.w {r4-r8,lr}
    mrs r8,APSR
    cmp r1,#0
    bne phase_passthrough
    ldr r4,phase_event
    cmp r0,r4
    bne phase_passthrough
    ldr r4,[r0,#28]
    ldr r5,phase_parameters
    cmp r4,r5
    bne phase_passthrough
    ldrb r5,[r4,#6]
    cmp r5,#1
    bne phase_passthrough
    ldrb r5,[r4,#7]
    cmp r5,#2
    bne phase_passthrough
    msr APSR_nzcvq,r8
    ldr r7,stock_phase
    blx r7
    mrs r8,APSR
    push.w {r0-r3,r12,lr}
    ldr r0,radio_control
    ldr r1,bis_address
    str r1,[r0,#4]
    ldr r1,bis_crc
    str r1,[r0,#0x38]
    ldr r1,[r0]
    orr r1,r1,#0x300000
    str r1,[r0]
    movs r1,#0
    str r1,[r0,#0x88]
    adr r4,bis_trace
    ldr r1,[r0]
    str r1,[r4,#4]
    ldr r1,[r0,#4]
    str r1,[r4,#8]
    ldr r1,[r0,#0x38]
    str r1,[r4,#12]
    ldr r1,[r0,#0x88]
    str r1,[r4,#16]
    dmb
    ldr r1,[r4]
    adds r1,#1
    str r1,[r4]
    pop.w {r0-r3,r12,lr}
    msr APSR_nzcvq,r8
    pop.w {r4-r8,pc}
phase_passthrough:
    msr APSR_nzcvq,r8
    pop.w {r4-r8,lr}
    ldr.w pc,stock_phase
    .balign 4
stock_phase: .word 0x00071547
phase_event: .word 0x00209788
phase_parameters: .word 0x002097b8
radio_control: .word 0x00318b6c
bis_address: .word 0x718ebed6
bis_crc: .word 0x00123401
    .space 768-(.-hook),0
bis_trace: .space 20
.ifdef RX_STREAM
    .space 912-(.-hook),0
.thumb_func
stream_forward:
    ldmia.w sp,{r0-r3}
    ldr r12,[sp,#40]
    msr APSR_nzcvq,r8
    ldr r7,stream_stock
    blx r7
    push.w {r0-r3,r12,lr}
    mrs r8,APSR
    bl stream_wake
    b.w rx_return
.thumb_func
stream_wake:
    adr.w r0,trace
    ldr r0,[r0]
    cmp r0,#2
    bne stream_wake_done
    ldr r0,transport_context
    ldr r0,[r0]
    cbz r0,stream_wake_done
    push {r4,lr}
    add.w r0,r0,#0xb4
    movs r1,#1
    movs r2,#0
    ldr r3,event_or
    blx r3
    pop {r4,pc}
stream_wake_done:
    bx lr
    .balign 4
stream_stock: .word 0x0007143f
transport_context: .word 0x00201f1c
event_or: .word 0x00009061
.endif
    .space 1024-(.-hook),0
.else
    .space 512-(.-hook),0
.endif
