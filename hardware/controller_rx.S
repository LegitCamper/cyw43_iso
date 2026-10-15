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
.ifdef RX_TIMER_HOP
    bl marker_acquire
.endif
    dmb
    movs r0,#2
    str r0,[r4]
.ifdef RX_HOP
    bl hop_advance
.endif
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
.ifdef RX_HOP
    bl hop_initial
.else
.ifdef RX_TIMER_HOP
    bl hop_timer_initial
.endif
.endif
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
.ifdef RX_HOP
    bl hop_advance
.endif
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
.ifdef RX_HOP
    .space 1024-(.-hook),0
.thumb_func
hop_advance:
    push {r4,lr}
    adr r4,hop_trace
    ldr r0,[r4]
    adds r0,#1
    uxth r0,r0
    str r0,[r4]
    bl configure_channel
    str r0,[r4,#4]
    ldr r0,[r4,#8]
    adds r0,#1
    str r0,[r4,#8]
    pop {r4,pc}
.thumb_func
hop_initial:
    push {r4,lr}
    adr r4,hop_trace
    movs r0,#0
    str r0,[r4]
    bl configure_channel
    str r0,[r4,#4]
    movs r0,#1
    str r0,[r4,#8]
    pop {r4,pc}
    .balign 4
hop_trace:
    .space 12
    .space 1152-(.-hook),0
.thumb_func
configure_channel:
    push {r4,lr}
    movw r1,#0xcf58
    bl bis_channel
    mov r2,r0
    ldr r1,whitening_channel
    str r2,[r1]
    lsls r1,r2,#1
    cmp r2,#10
    ite ls
    addls r1,r1,#2
    addhi r1,r1,#4
    orr r1,r1,#0x80
    ldr r0,rf_channel
    str r1,[r0]
    mov r0,r2
    pop {r4,pc}
    .balign 4
whitening_channel: .word 0x00318bf4
rf_channel: .word 0x003186a4
    .space 1280-(.-hook),0
.thumb_func
bis_channel:
    .incbin "work/bis-channel/bis_channel.bin"
    .space 2048-(.-hook),0
.else
.ifdef RX_TIMER_HOP
    .space 1024-(.-hook),0
.thumb_func
hop_advance:
    push {r4,lr}
    adr r4,hop_trace
    ldr r0,[r4]
    adds r0,#1
    uxth r0,r0
    str r0,[r4]
    bl configure_channel
    str r0,[r4,#4]
    ldr r0,[r4,#8]
    adds r0,#1
    str r0,[r4,#8]
    pop {r4,pc}
.thumb_func
hop_timer_initial:
    push {r4,lr}
    adr r4,hop_trace
    movs r0,#0
    str r0,[r4]
    bl configure_channel
    str r0,[r4,#4]
    movs r0,#1
    str r0,[r4,#8]
    movs r0,#32
    str r0,[r4,#12]
    movs r0,#0
    str r0,[r4,#16]
    str r0,[r4,#20]
    str r0,[r4,#24]
    str r0,[r4,#28]
    adr r0,timer_countdown
    orr r0,r0,#1
    adr r1,callbacks
    str r0,[r1,#36]
    pop {r4,pc}
    .balign 4
hop_trace:
    .space 32
    .space 1132-(.-hook),0
.thumb_func
timer_countdown:
    push {r4-r8,lr}
    mov r4,r0
    ldr r5,stock_countdown
    blx r5
    ldr r5,timer_event
    cmp r4,r5
    bne timer_done
    ldr r5,[r4,#28]
    ldr r6,timer_parameters
    cmp r5,r6
    bne timer_done
    ldrb r5,[r5,#6]
    cmp r5,#1
    bne timer_done
    mov r7,r4
    adr r4,hop_trace
    ldr r5,[r4,#16]
    adds r5,#1
    str r5,[r4,#16]
    ldrb r5,[r7,#15]
    cmp r5,#1
    bne timer_done
    ldr r5,[r4,#24]
    cbnz r5,timer_synchronized
    bl hop_advance
    mov r0,r7
    bl timer_rearm
    ldr r5,[r4,#28]
    adds r5,#1
    str r5,[r4,#28]
    b timer_done
timer_synchronized:
    ldr r5,[r4,#12]
    subs r5,#1
    str r5,[r4,#12]
    bne timer_done
    movs r5,#32
    str r5,[r4,#12]
    bl hop_advance
    mov r0,r7
    bl timer_rearm
    ldr r5,[r4,#20]
    adds r5,#1
    str r5,[r4,#20]
timer_done:
    pop {r4-r8,pc}
.thumb_func
timer_rearm:
    push {r4,r5,r6,lr}
    mov r4,r0
    ldr.w pc,stock_rx_rearm
    .balign 4
stock_countdown: .word 0x000713fd
timer_event: .word 0x00209788
timer_parameters: .word 0x002097b8
stock_rx_rearm: .word 0x000714a5
    .space 1280-(.-hook),0
.thumb_func
configure_channel:
    push {r4,lr}
    movw r1,#0xcf58
    bl bis_channel
    mov r2,r0
    ldr r1,whitening_channel
    str r2,[r1]
    lsls r1,r2,#1
    cmp r2,#10
    ite ls
    addls r1,r1,#2
    addhi r1,r1,#4
    orr r1,r1,#0x80
    ldr r0,rf_channel
    str r1,[r0]
    mov r0,r2
    pop {r4,pc}
    .balign 4
whitening_channel: .word 0x00318bf4
rf_channel: .word 0x003186a4
    .space 1408-(.-hook),0
.thumb_func
bis_channel:
    .incbin "work/bis-channel/bis_channel.bin"
    .space 1536-(.-hook),0
.thumb_func
marker_acquire:
    push {r4-r8,lr}
    adr r4,hop_trace
    ldr r0,[r4,#24]
    cbnz r0,marker_done
    adr r5,trace
    add.w r5,r5,#48
    movs r1,#2
marker_validate:
    ldrb r2,[r5,r1]
    cmp r2,r1
    bne marker_done
    adds r1,#1
    cmp r1,#26
    bne marker_validate
    mrs r7,PRIMASK
    cpsid i
    ldr r0,[r4,#24]
    cbnz r0,marker_unlock
    ldrh r0,[r5]
    str r0,[r4]
    bl configure_channel
    str r0,[r4,#4]
    ldr r0,[r4,#8]
    adds r0,#1
    str r0,[r4,#8]
    movs r0,#32
    str r0,[r4,#12]
    movs r0,#1
    dmb
    str r0,[r4,#24]
marker_unlock:
    msr PRIMASK,r7
marker_done:
    pop {r4-r8,pc}
    .space 2048-(.-hook),0
.else
    .space 1024-(.-hook),0
.endif
.endif
.else
    .space 512-(.-hook),0
.endif
