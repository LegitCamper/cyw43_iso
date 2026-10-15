.syntax unified
.cpu cortex-m3
.thumb
.section .text
.global hook
.thumb_func
hook:
    push.w {r4-r8,lr}
    mrs r8,APSR
    cmp r1,#0
    bne passthrough
    ldr r4,event_address
    cmp r0,r4
    bne passthrough
    ldr r4,[r0,#28]
    ldr r5,parameter_address
    cmp r4,r5
    bne passthrough
    ldrb r5,[r4,#5]
    cmp r5,#31
    bne passthrough
    ldrb r5,[r4,#6]
    cmp r5,#0
    bne passthrough
    adr r6,trace
    ldr r5,[r6]
    cmp r5,#0
    bne passthrough
    ldr r5,[r4]
    str r5,[r6,#4]
    adr r7,payload
    str r7,[r4]
    movs r7,#1
    str r7,[r6]
    msr APSR_nzcvq,r8
    ldr r7,stock_phase
    blx r7
    mrs r8,APSR
    push {r0-r3}
.ifdef BIS_PACKET
    bl configure_bis
.endif
    ldr r0,payload_window
    adr r1,snapshot
    movs r2,#10
copy_snapshot:
    ldr r3,[r0],#4
    str r3,[r1],#4
    subs r2,#1
    bne copy_snapshot
    pop {r0-r3}
    str r5,[r4]
    movs r7,#2
    str r7,[r6]
    msr APSR_nzcvq,r8
    pop.w {r4-r8,pc}
passthrough:
    msr APSR_nzcvq,r8
    pop.w {r4-r8,lr}
    ldr.w pc,stock_phase
    .balign 4
stock_phase: .word 0x00071547
event_address: .word 0x00209788
parameter_address: .word 0x002097b8
payload_window: .word 0x00370800
    .space 160-(.-hook),0
.thumb_func
tx_complete:
    push {r4-r6,lr}
    mrs r6,APSR
    adr r4,trace
    ldr r5,[r4,#8]
    adds r5,#1
    str r5,[r4,#8]
    mrs r5,IPSR
    str r5,[r4,#12]
    msr APSR_nzcvq,r6
    pop.w {r4-r6,lr}
    ldr.w pc,stock_tx
    .balign 4
stock_tx: .word 0x000714cf
    .space 224-(.-hook),0
callbacks:
    .space 44
    .space 272-(.-hook),0
trace:
    .space 32
    .space 320-(.-hook),0
payload:
    .byte 0,1,2,3,4,5,6,7,8,9,10,11,12,13,14,15
    .byte 16,17,18,19,20,21,22,23,24,25,26,27,28,29,30,31
    .byte 32,33,34,35,36,37,38,39
    .space 368-(.-hook),0
snapshot:
    .space 40
.ifdef BIS_PACKET
    .space 432-(.-hook),0
.thumb_func
configure_bis:
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
    bx lr
    .balign 4
radio_control: .word 0x00318b6c
bis_address: .word 0x718ebed6
bis_crc: .word 0x00123401
.endif
    .space 512-(.-hook),0
