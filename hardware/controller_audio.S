.syntax unified
.cpu cortex-m3
.thumb
.section .text
.global hook
image_base:
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
    cmp r5,#26
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
.ifdef DISCOVERY_PACKET
    movs r7,#6
.else
    movs r7,#16
.endif
    strh r7,[r4,#8]
    msr APSR_nzcvq,r8
    ldr r7,stock_phase
    blx r7
    mrs r8,APSR
    push {r0-r3}
.ifdef DISCOVERY_PACKET
    adr r0,image_base
    movs r1,#0
    bl prepare_discovery
.else
    bl configure_bis
.endif
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
    .space 128-(.-hook),0
.thumb_func
tx_complete:
    push {r4-r6,lr}
    ldr r4,stock_tx
    blx r4
    mrs r6,APSR
    push {r0-r3}
    adr r4,trace
    ldr r5,[r4,#8]
    adds r5,#1
    str r5,[r4,#8]
    mrs r1,IPSR
    str r1,[r4,#12]
    ldr r1,event_address
.ifdef DISCOVERY_PACKET
    ldr r2,[r1,#28]
    ldr r3,parameter_address
    cmp r2,r3
    bne complete_done
.endif
    ldr r1,[r1,#32]
    cbnz r1,complete_done
    ldr r1,[r4]
    cmp r1,#2
    bne complete_done
.ifdef DISCOVERY_PACKET
    adr r0,image_base
    mov r1,r5
    bl prepare_discovery
.else
    and r5,r5,#3
    movs r1,#28
    muls r5,r1
    adr r2,payload
    adds r2,r5
    movs r0,#0
    movw r1,#0x1a00
    movs r3,#0
    sub sp,#8
    str r3,[sp]
    ldr r4,stock_copy
    blx r4
    add sp,#8
.ifdef HOP_PACKET
    adr r4,trace
    ldr r0,[r4,#8]
    bl configure_channel
.endif
.endif
complete_done:
    pop {r0-r3}
    msr APSR_nzcvq,r6
    pop {r4-r6,pc}
    .balign 4
stock_tx: .word 0x000714cf
stock_copy: .word 0x00044ed3
    .space 224-(.-hook),0
callbacks:
    .space 44
    .space 272-(.-hook),0
trace:
    .space 32
    .space 320-(.-hook),0
payload:
    .byte 0,10,62,106,45,168,181,80,63,252,172,220,237,137,35,143,3,94,77,126,89,237,121,231,110,163
    .space 2
    .byte 4,144,97,55,26,236,157,82,112,161,126,43,15,171,165,85,74,85,85,169,131,124,233,238,113,158
    .space 2
    .byte 252,130,45,163,39,150,250,163,245,133,106,53,58,61,64,253,41,90,25,14,94,187,111,73,237,153
    .space 2
    .byte 115,85,243,82,251,178,152,91,79,139,194,88,235,85,115,130,169,88,26,2,212,190,150,197,236,167
    .space 2
    .space 12
    .space 448-(.-hook),0
.thumb_func
configure_bis:
.ifdef HOP_PACKET
    push {r4,lr}
.endif
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
.ifdef HOP_PACKET
    movs r0,#0
    bl configure_channel
    pop {r4,pc}
.else
    bx lr
.endif
    .balign 4
radio_control: .word 0x00318b6c
bis_address: .word 0x718ebed6
bis_crc: .word 0x00123401
.ifdef HOP_PACKET
    .space 512-(.-hook),0
.thumb_func
configure_channel:
    push {r4,lr}
    movw r1,#0xcf58
    bl bis_channel
    ldr r1,whitening_channel
    str r0,[r1]
    lsls r1,r0,#1
    cmp r0,#10
    ite ls
    addls r1,r1,#2
    addhi r1,r1,#4
    orr r1,r1,#0x80
    ldr r0,rf_channel
    str r1,[r0]
    pop {r4,pc}
    .balign 4
whitening_channel: .word 0x00318bf4
rf_channel: .word 0x003186a4
    .space 640-(.-hook),0
.thumb_func
bis_channel:
    .incbin "work/bis-channel/bis_channel.bin"
.ifdef DISCOVERY_PACKET
    .space 1024-(.-hook),0
.ifdef CONTINUOUS_ISO_PACKET
    .incbin "work/discovery-continuous/discovery-primary.bin"
    .space 1088-(.-hook),0
    .incbin "work/discovery-continuous/discovery-auxiliary.bin"
    .space 1152-(.-hook),0
    .incbin "work/discovery-continuous/discovery-periodic.bin"
.else
    .incbin "work/discovery-primary.bin"
    .space 1088-(.-hook),0
    .incbin "work/discovery-auxiliary.bin"
    .space 1152-(.-hook),0
    .incbin "work/discovery-periodic.bin"
.endif
    .space 1280-(.-hook),0
.thumb_func
prepare_discovery:
.ifdef NATIVE_ISO_PACKET
    b.w consume_discovery
.else
.ifdef SCAN_PACKET
    .incbin "work/discovery-scan-packet/discovery_packet.bin"
.else
    .incbin "work/discovery-packet/discovery_packet.bin"
.endif
.endif
.ifdef NATIVE_ISO_PACKET
    .space 2048-(.-hook),0
    .incbin "work/transport-ingress/transport.bin"
    .space 3072-(.-hook),0
    .incbin "work/iso-credits.bin"
    .space 3328-(.-hook),0
.thumb_func
consume_discovery:
.ifdef CONTINUOUS_ISO_PACKET
    .incbin "work/discovery-continuous-packet/discovery_packet.bin"
.else
    .incbin "work/discovery-native-packet/discovery_packet.bin"
.endif
    .space 4096-(.-hook),0
.else
    .space 2048-(.-hook),0
.endif
.else
    .space 1024-(.-hook),0
.endif
.else
    .space 512-(.-hook),0
.endif
