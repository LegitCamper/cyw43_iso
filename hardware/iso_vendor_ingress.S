/* Experimental vendor command 0xfcff, not standard HCI ISO transport.
 * Install this image at resident+1632; metadata begins at +1792 and slots
 * at +1824. Single HCI-task producer, single radio-ISR consumer.
 * Consumer copies before DMB/tail publication; no command pointer is retained.
 */
.syntax unified
.cpu cortex-m3
.thumb
.section .text
.global vendor_lookup, vendor_handler, vendor_metadata, vendor_slots, vendor_guard

vendor_image:
.thumb_func
vendor_lookup:
    mrs r12,APSR
    cmp r3,#0x3f
    bne stock_lookup
    cmp r2,#0xff
    bne stock_lookup
    /* NOTYPE alias avoids ADR adding the Thumb bit implicitly. */
    adr r2,handler_address
    adds r2,#1
    str r2,[r1]
    ldr r2,descriptor
    str r2,[r1,#4]
    bx lr
stock_lookup:
    msr APSR_nzcvq,r12
    ldr.w pc,stock_callback
    .balign 4
descriptor: .word 0x0006001d
stock_callback: .word 0x000d035d

handler_address:
.thumb_func
vendor_handler:
    push {r4-r7}
    mov r6,r2
    adr r4,vendor_metadata
    ldrb r2,[r0,#11]
    cmp r2,#26
    bne invalid
    ldr r5,[r4]
    ldr r7,[r4,#4]
    cmp r5,#4
    bhi invalid
    cmp r7,#4
    bhi invalid
    adds r3,r5,#1
    cmp r3,#5
    it eq
    moveq r3,#0
    cmp r3,r7
    beq full
    add.w r5,r5,r5,lsl #2
    add.w r5,r4,r5,lsl #3
    adds r5,#32
    adds r0,#12
    movs r7,#26
copy:
    subs r7,#1
    ldrb r1,[r0,r7]
    strb r1,[r5,r7]
    bne copy
    dmb sy
    str r3,[r4]
    ldr r0,[r4,#8]
    adds r0,#1
    str r0,[r4,#8]
    movs r0,#0
    b reply
full:
    movs r0,#7
    b reject
invalid:
    movs r0,#0x12
reject:
    ldr r1,[r4,#12]
    adds r1,#1
    str r1,[r4,#12]
reply:
    strb r0,[r6,#5]
    pop {r4-r7}
    bx lr
    .balign 4
code_end:
    .space 160-(.-vendor_image),0
vendor_metadata:
    /* head, tail, accepted, rejected, consumed, underrun, reserved[2] */
    .space 32,0
vendor_slots:
    /* Five 40-byte slots: 26 payload bytes, 14 untouched zero padding bytes. */
    .space 200,0
vendor_guard:
    .space 24,0
