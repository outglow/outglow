@ Startup for the Telink TLSR8258 (TC32 core), written for this project.
@
@ Assembled by the llvm-tc32 clang (-mcpu=tc32): Thumb mnemonics are written
@ in TC32's encoding. The TC32-only instructions have no Thumb mnemonic and
@ are given as the Thumb values the assembler maps to them:
@   tmcsr rN   0xbbc0 + N   CPSR = rN
@   tmrss rN   0xbbd8 + N   rN = SPSR
@   tmssr rN   0xbbd0 + N   SPSR = rN
@   treti {pc} 0xb900       pop pc, CPSR = SPSR
@
@ Image header (what the boot ROM and Telink's OTA code read):
@   0x00  branch to the reset code
@   0x08  "KNLT"
@   0x0c  0x0088 in the high half; low half: bytes / 16 the boot ROM copies
@         from the start of the image to SRAM 0x840000 (vectors + .ram_code)
@   0x10  branch to the interrupt entry
@   0x12  manufacturer code, image type
@   0x18  image size in bytes
@
@ SRAM, from 0x840000: the RAM copy (256-byte aligned), 0x100 bytes of flash
@ cache tags, 0x800 bytes of flash cache data, then .data and .bss; the
@ interrupt stack is the top 1 KB (below 0x850000), the main stack below it.

	.syntax unified
	.thumb

	.equ	MODE_IRQ, 0x12
	.equ	MODE_SVC, 0x13

@ Telink's manufacturer code, as in the stock image's header; an image type of
@ this project's own, so that no OTA server takes the image for another one.
#ifndef MANUFACTURER_CODE
#define MANUFACTURER_CODE 0x1141
#endif
#ifndef IMAGE_TYPE
#define IMAGE_TYPE 0x0390
#endif

	.section .vectors, "ax", %progbits
	.global	__start
	.global	__reset
	.global	__irq
	.type	__start, %function
	.thumb_func
__start:
	b	__reset
	.short	0
	.word	0
	.word	0x544c4e4b			@ 0x08 "KNLT"
	.word	0x00880000 + _ramcode_size_div_16_align_256_	@ 0x0c
	b	__irq				@ 0x10
	.short	MANUFACTURER_CODE		@ 0x12
	.short	IMAGE_TYPE			@ 0x14
	.short	0
	.word	_bin_size_			@ 0x18
	.word	0

	.type	__reset, %function
	.thumb_func
__reset:
	@ stack pointers of the IRQ and SVC modes
	movs	r0, #MODE_IRQ
	.inst.n	0xbbc0				@ tmcsr r0
	ldr	r0, =_irq_stack_top_
	mov	sp, r0
	movs	r0, #MODE_SVC
	.inst.n	0xbbc0				@ tmcsr r0
	ldr	r0, =_stack_end_
	mov	sp, r0

	@ flash cache: clear the tags, then tell the cache where they are
	movs	r0, #0
	ldr	r1, =_ictag_start_
	ldr	r2, =_ictag_end_
1:	cmp	r1, r2
	bhs	2f
	str	r0, [r1]
	adds	r1, #4
	b	1b
2:	ldr	r1, =0x80060c
	ldr	r0, =_ramcode_size_align_256_
	lsrs	r0, r0, #8
	strb	r0, [r1]			@ 0x60c: tag location / 256
	adds	r0, #1
	strb	r0, [r1, #1]			@ 0x60d: cache data location / 256

	@ release every block from reset, enable every clock gate
	ldr	r0, =0x800060
	ldr	r1, =0xff000000
	str	r1, [r0]			@ 0x60-0x62 = 0 (no reset), 0x63 = 0xff
	movs	r1, #0xff
	strb	r1, [r0, #4]			@ 0x64
	strb	r1, [r0, #5]			@ 0x65

	@ wake the flash from deep power-down (command 0xab on the MSPI)
	ldr	r0, =0x80000c
	movs	r1, #0
	strb	r1, [r0, #1]			@ chip select low
	movs	r1, #0xab
	strb	r1, [r0]
	movs	r2, #0
3:	adds	r2, #1
	cmp	r2, #6
	ble	3b
	movs	r1, #1
	strb	r1, [r0, #1]			@ chip select high

	@ .data from flash, .bss to zero
	ldr	r1, =_dstored_
	ldr	r2, =_start_data_
	ldr	r3, =_end_data_
4:	cmp	r2, r3
	bhs	5f
	ldr	r0, [r1]
	str	r0, [r2]
	adds	r1, #4
	adds	r2, #4
	b	4b
5:	movs	r0, #0
	ldr	r1, =_start_bss_
	ldr	r2, =_end_bss_
6:	cmp	r1, r2
	bhs	7f
	str	r0, [r1]
	adds	r1, #4
	b	6b
	@ both stacks (and the free RAM between .bss and them) to a known
	@ pattern, so that how deep they ever went can be read off later
	@ (stack.rs); nothing has been pushed yet, interrupts are off
7:	ldr	r0, =0xa5a5a5a5
	ldr	r1, =_end_bss_
	ldr	r2, =_irq_stack_top_
9:	cmp	r1, r2
	bhs	10f
	str	r0, [r1]
	adds	r1, #4
	b	9b
10:	bl	main
8:	b	8b
	.pool

	@ Interrupt entry: save what the handler may change (r0-r12, lr and the
	@ interrupted CPSR, which is in SPSR), call irq_handler, return with treti.
	.type	__irq, %function
	.thumb_func
__irq:
	push	{lr}
	push	{r0-r7}
	.inst.n	0xbbd8				@ tmrss r0
	mov	r1, r8
	mov	r2, r9
	mov	r3, r10
	mov	r4, r11
	mov	r5, r12
	push	{r0-r5}
	bl	irq_handler
	pop	{r0-r5}
	mov	r8, r1
	mov	r9, r2
	mov	r10, r3
	mov	r11, r4
	mov	r12, r5
	.inst.n	0xbbd0				@ tmssr r0
	pop	{r0-r7}
	.inst.n	0xb900				@ treti {pc}
