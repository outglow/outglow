/* Link script for the Rust firmware on the TLSR8258 (see boot/start.S).
 *
 * The first _ramcode_size_align_256_ bytes of the image are copied to SRAM
 * and run at address 0 upwards (they must not depend on the flash: the flash
 * driver and the radio interrupt live in .ram_code). The rest runs in place
 * from the flash, where the image sits at FW_OFFSET (given by `cargo xtask
 * build` with --defsym, from the board): 0x6A000 on the bulbs, the address
 * their resident bootloader installs applications at and the stock firmware
 * is linked for (docs/ble-first-flash.md), 0 on a bare module the boot ROM
 * starts directly. In the file the two parts follow each other without a
 * gap. */
ENTRY(__start)

SECTIONS
{
    . = 0x0;
    .vectors : { KEEP(*(.vectors)) KEEP(*(.vectors.*)) }
    .ram_code : { *(.ram_code) *(.ram_code.*) }
    . = ALIGN(16);
    _rstored_ = .;
    _ramcode_size_div_16_align_256_ = ((. + 255) / 256) * 16;
    _ramcode_size_align_256_ = ((. + 255) / 256) * 256;

    . = _rstored_ + FW_OFFSET;
    .text : AT(_rstored_) { *(.text) *(.text.*) }
    .rodata : { *(.rodata) *(.rodata.*) }
    . = ALIGN(4);
    /* where the initial values of .data are: as an address, and in the file */
    _dstored_ = .;
    _dstored_bin_ = . - FW_OFFSET;

    /* SRAM: RAM copy | 0x100 cache tags | 0x800 cache data | data | bss */
    _ictag_start_ = 0x840000 + _ramcode_size_align_256_;
    _ictag_end_ = _ictag_start_ + 0x100;
    . = 0x840900 + _ramcode_size_align_256_;
    .data : AT(_dstored_bin_)
    {
        . = ALIGN(4);
        _start_data_ = .;
        *(.data) *(.data.*)
        . = ALIGN(4);
        _end_data_ = .;
    }
    /* Left alone by the start-up code (neither copied nor zeroed): what a
     * watchdog or software reset must not clear (zb.rs, `Retained`). */
    .noinit (NOLOAD) :
    {
        . = ALIGN(4);
        *(.noinit) *(.noinit.*)
        . = ALIGN(4);
    }
    .bss (NOLOAD) :
    {
        . = ALIGN(4);
        _start_bss_ = .;
        *(.bss) *(.bss.*) *(COMMON)
        . = ALIGN(4);
        _end_bss_ = .;
    }
    _bin_size_ = _dstored_bin_ + (_end_data_ - _start_data_);
    /* The image must fit the OTA staging area (400 KB): past it, from
     * FW_OFFSET + 0x64000, the flash holds data (the power-on counter, the
     * settings sectors; src/board.rs). A bulb takes an update only when the
     * whole .ota file (its header, the sub-element header, the image padded
     * to 16 bytes and its CRC) fits, so the limit is IMAGE_LIMIT, given by
     * `cargo xtask build` with --defsym (outglow_image::ota::IMAGE_LIMIT). */
    ASSERT(_bin_size_ <= IMAGE_LIMIT, "the image is too large for an OTA file that fits the 400 KB OTA staging area")

    /* Stacks: the interrupt stack is the top 1 KB; the main (SVC) stack
     * grows down from below it towards .bss. */
    _irq_stack_top_ = 0x850000;
    _stack_end_ = _irq_stack_top_ - 0x400;
    ASSERT(_end_bss_ + 0x4000 < _stack_end_, "SRAM: less than 16 KB left for the main stack")

    /DISCARD/ : { *(.ARM.exidx*) *(.ARM.extab*) *(.comment) *(.note*) }
}
