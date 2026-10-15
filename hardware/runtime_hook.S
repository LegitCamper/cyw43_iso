.syntax unified
.cpu cortex-m3
.thumb
.section .text
.global hook
.thumb_func
hook:
.ifdef SRAM_EXEC_ALLOC
    push.w {r0-r9,r12,lr}
    mrs r8,APSR
    mrs r7,PRIMASK
    cpsid i
    ldr r4,trace_address
    ldr r0,[r4]
    cbnz r0,allocation_done
    movs r0,#1
    str r0,[r4]
    mrs r0,IPSR
    cbnz r0,allocation_done
    ldr r5,bounds_address
    ldrb r0,[r5]
    cmp r0,#1
    bne allocation_done
    ldr r0,[r5,#4]
    ldr r1,[r4,#4]
    cmp r0,r1
    bne allocation_done
    ldr r0,[r5,#8]
    ldr r1,[r4,#8]
    cmp r0,r1
    bne allocation_done
.ifndef SRAM_RESERVATION_SIZE
    .set SRAM_RESERVATION_SIZE,512
.endif
    mov.w r0,#SRAM_RESERVATION_SIZE
    bl arena_allocate
    cbz r0,allocation_done
    str r0,[r4]
allocation_done:
    msr PRIMASK,r7
    msr APSR_nzcvq,r8
    pop.w {r0-r9,r12,lr}
    b.w stock_hook
    .balign 4
trace_address:
    .word 0x000d0ff0
bounds_address:
    .word 0x00200560
    .space 96-(.-hook),0
    .space 16
.else
.ifdef SRAM_EXEC_PAYLOAD
    push {r4,r5}
    adr r4,sram_trace
    ldr r5,[r4]
    add.w r5,r5,#1
    str r5,[r4]
    str r0,[r4,#4]
    str r1,[r4,#8]
    str r2,[r4,#12]
    str r3,[r4,#16]
    pop {r4,r5}
    ldr.w pc,sram_stock
    .balign 4
sram_stock:
    .word 0x000d035d
sram_trace:
    .space 20
.else
.ifdef EVENT_PHASE_TIMING
    push.w {r4-r8,lr}
    mov r4,r0
    mov r5,r1
    bl stock_hook
    mrs r8,APSR
    cmp r5,#2
    bne timing_return
    ldr r6,[r4,#0x1c]
    ldr.w r7,[r4,#0xa8]
    sub.w r6,r6,r7
    ubfx r6,r6,#0,#27
    ldr r7,trace_address
    str r6,[r7]
timing_return:
    msr APSR_nzcvq,r8
    pop.w {r4-r8,pc}
    .balign 4
trace_address:
    .word 0x000d0fd0
    .space 64-(.-hook),0
.else
.ifdef EVENT_PHASE_PROBE
    push {r4-r7}
    mrs r7,APSR
    ldr r4,trace_address
    mov r5,r1
    cmp r5,#2
    it hi
    movhi r5,#3
    ldr.w r6,[r4,r5,lsl #2]
    add.w r6,r6,#1
    str.w r6,[r4,r5,lsl #2]
    str r0,[r4,#16]
    msr APSR_nzcvq,r7
    pop {r4-r7}
    b.w stock_hook
    .balign 4
trace_address:
    .word 0x000d0fc0
    .space 48-(.-hook),0
.else
    push {r4,r5}
    ldr r4, trace_address
    ldr r5, [r4]
    add.w r5,r5,#1
    str r5,[r4]
    str r0,[r4,#4]
    str r1,[r4,#8]
    str r2,[r4,#12]
    str r3,[r4,#16]
    pop {r4,r5}
.ifdef STATE7_PROBE
    push.w {r4-r8,lr}
.endif
    b.w stock_hook
    .balign 4
trace_address:
.ifdef STATE7_PROBE
    .word 0x000d0fb4
.else
    .word 0x000d0fb0
.endif
.endif
.endif
    .global stock_hook
    .space 20
.endif
.endif
