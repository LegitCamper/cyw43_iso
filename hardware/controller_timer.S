.syntax unified
.cpu cortex-m3
.thumb
.section .text
.global hook
.thumb_func
hook:
    push.w {r0-r9,r12,lr}
    mrs r8,APSR
    mrs r7,PRIMASK
    cpsid i
    adr r4,trace
    mrs r0,IPSR
    cmp r0,#0
    bne done
    ldr r0,[r4]
    cmp r0,#2
    beq cancel
    cmp r0,#1
    bne done
    ldr r0,[r4,#4]
    cbnz r0,done
    movs r0,#1
    str r0,[r4,#4]
    adr r0,timer
    adr r1,tick
    orr r1,r1,#1
    mov r2,r4
    ldr r3,period
    ldr r5,initialize
    blx r5
    adr r0,timer
    ldr r1,period
    ldr r5,arm
    blx r5
    movs r0,#2
    str r0,[r4,#4]
    b done
cancel:
    ldr r0,[r4,#4]
    cmp r0,#2
    bne done
    adr r0,timer
    ldr r5,remove
    blx r5
    str r0,[r4,#24]
    movs r0,#3
    str r0,[r4,#4]
done:
    msr PRIMASK,r7
    msr APSR_nzcvq,r8
    pop.w {r0-r9,r12,lr}
    ldr.w pc,stock
.thumb_func
tick:
    ldr r2,[r0,#8]
    adds r2,#1
    str r2,[r0,#8]
    mrs r2,IPSR
    str r2,[r0,#12]
    str r1,[r0,#16]
    str r0,[r0,#20]
    bx lr
    .balign 4
initialize: .word 0x0000742d
arm: .word 0x000073d5
remove: .word 0x00007409
period: .word 125000
stock: .word 0x000d035d
    .space 192-(.-hook),0
trace:
    .space 32
timer:
    .space 32
