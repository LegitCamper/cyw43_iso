/* ponytail: one diagnostic handle 0x0042; add per-stream state for negotiated streams.
 * Task context only. No RF completion is inferred.
 * iso_emit_completed(count) returns 1 after ROM takes ownership, 0 on invalid
 * count/allocation failure. Caller retains pending credits until return 1.
 */
.syntax unified
.cpu cortex-m3
.thumb
.section .text
.global iso_credit_lookup, iso_buffer_size_v2, iso_emit_completed
.global iso_credit_previous_lookup

.thumb_func
iso_credit_lookup:
    mrs r12,APSR
    cmp r3,#8
    bne passthrough
    cmp r2,#0x60
    bne passthrough
    adr r2,buffer_handler_address
    adds r2,#1
    str r2,[r1]
    ldr r2,descriptor
    str r2,[r1,#4]
    bx lr
passthrough:
    msr APSR_nzcvq,r12
    ldr.w pc,iso_credit_previous_lookup
    .balign 4
descriptor: .word 0x000c0003
iso_credit_previous_lookup: .word 0x000d035d

buffer_handler_address:
.thumb_func
iso_buffer_size_v2:
    push {r4,lr}
    mov r4,r2
    ldrb r3,[r0,#11]
    cmp r3,#0
    bne bad_length
    ldr r3,stock_buffer_handler
    blx r3
    movs r0,#0
    strb r0,[r4,#5]
    movs r0,#30
    strh r0,[r4,#9]
    movs r0,#4
    strb r0,[r4,#11]
    pop {r4,pc}
bad_length:
    movs r0,#0
    strh r0,[r4,#6]
    strb r0,[r4,#8]
    strh r0,[r4,#9]
    strb r0,[r4,#11]
    movs r0,#0x12
    strb r0,[r4,#5]
    pop {r4,pc}

.thumb_func
iso_emit_completed:
    push {r4,lr}
    mov r4,r0
    subs r0,#1
    cmp r0,#3
    bhi failed
    movs r0,#7
    movs r1,#0x13
    movs r2,#5
    ldr r3,event_allocate
    blx r3
    cbz r0,failed
    movs r1,#1
    strb r1,[r0,#10]
    movs r1,#0x42
    strh r1,[r0,#11]
    strh r4,[r0,#13]
    ldr r3,event_send
    blx r3
    movs r0,#1
    pop {r4,pc}
failed:
    movs r0,#0
    pop {r4,pc}
    .balign 4
stock_buffer_handler: .word 0x00051745
event_allocate: .word 0x00041215
event_send: .word 0x00040e53
