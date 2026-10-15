.syntax unified
.cpu cortex-m3
.thumb
.section .text
.global hook
.thumb_func
hook:
    push.w {r0-r9,r12,lr}
    mrs r8,APSR
    adr r4,trace
    ldr r0,[r4]
    cmp r0,#0
    bne done
    mrs r0,IPSR
    str r0,[r4,#16]
    cmp r0,#0
    bne done
    mrs r0,PRIMASK
    str r0,[r4,#20]
    cmp r0,#0
    bne done
    ldr r0,current_thread
    ldr r0,[r0]
    str r0,[r4,#24]
    cmp r0,#0
    beq done
    ldr r5,hook_base
    ldr r0,[r5]
    ldr r1,[r5,#16]
    orrs r0,r1
    bne done
    ldr r5,scheduler
    ldr r0,[r5,#20]
    cmp r0,#0
    bne done
    movs r0,#1
    str r0,[r4]
    adr r5,event
    adr r6,callbacks
    adr r0,cleanup
    orr r0,r0,#1
    str r0,[r6,#8]
    adr r0,complete
    orr r0,r0,#1
    str r0,[r6,#36]
    movs r0,#0x30
    strb r0,[r5,#16]
    movs r0,#0x71
    strb r0,[r5,#18]
    movs r0,#2
    strb r0,[r5,#19]
    ldr r7,lock
    blx r7
    cpsid i
    movs r0,#0
    ldr r7,clock
    blx r7
    bic r0,r0,#1
    adds r0,#32
    ubfx r0,r0,#0,#27
    str r0,[r5,#28]
    str r0,[r5,#32]
    str r0,[r4,#28]
    cpsie i
    ldr r7,unlock
    blx r7
    mov r0,r5
    mov r1,r6
    movs r2,#3
    ldr r7,register_event
    blx r7
    movs r0,#2
    str r0,[r4]
done:
    msr APSR_nzcvq,r8
    pop.w {r0-r9,r12,lr}
    ldr.w pc,stock
.thumb_func
complete:
    push {r4-r6,lr}
    mov r4,r0
    ldr r5,is_current
    blx r5
    cbz r0,complete_done
    mov r0,r4
    ldr r5,is_ready
    blx r5
    cbz r0,complete_done
    adr r5,trace
    ldr r0,[r5,#4]
    adds r0,#1
    str r0,[r5,#4]
    mrs r0,IPSR
    str r0,[r5,#12]
    mov r0,r4
    ldr r5,acknowledge
    blx r5
    mov r0,r4
    ldr r5,remove
    blx r5
complete_done:
    pop {r4-r6,pc}
.thumb_func
cleanup:
    adr r1,trace
    ldr r2,[r1,#8]
    adds r2,#1
    str r2,[r1,#8]
    ldr.w pc,clear
    .balign 4
clock: .word 0x0000a561
register_event: .word 0x0001c61f
is_current: .word 0x0001cf4f
is_ready: .word 0x0001cb1d
acknowledge: .word 0x0001d5b9
remove: .word 0x0001c97f
clear: .word 0x00031139
stock: .word 0x000d035d
hook_base: .word 0x00208e28
scheduler: .word 0x002042b8
current_thread: .word 0x00201fe4
lock: .word 0x0000b4af
unlock: .word 0x0000b1df
    .space 352-(.-hook),0
callbacks:
    .word 0,0,0,0,0x0003113f,0,0,0,0,0,0x00031207
    .space 400-(.-hook),0
trace:
    .space 32
event:
    .space 40
    .space 512-(.-hook),0
