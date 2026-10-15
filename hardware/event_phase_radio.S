.syntax unified
.cpu cortex-m3
.thumb
.section .text
.global hook
.thumb_func
hook:
    push {r4-r6,lr}
    mov r4,r1
    bl stock_hook
    push {r0,r1}
    mrs r6,APSR
    cmp r4,#1
    bne done
    ldr r4,mmio
    adr r5,trace
    ldr r0,[r4,#0x714]
    str r0,[r5]
    ldr r0,[r4,#0xb68]
    str r0,[r5,#4]
done:
    msr APSR_nzcvq,r6
    pop {r0,r1}
    pop {r4-r6,pc}
    .balign 4
mmio: .word 0x00318000
    .space 48-(.-hook),0
trace:
