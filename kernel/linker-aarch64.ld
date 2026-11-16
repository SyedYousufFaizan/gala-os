/* Kernel image layout on aarch64: as linker.ld (x86-64), with its own
   format and entry (arch::aarch64::entry turns FP/SIMD on, then calls kmain). */
OUTPUT_FORMAT(elf64-littleaarch64)
ENTRY(gala_aarch64_entry)

PHDRS
{
    rodata PT_LOAD FLAGS(4);    /* R */
    text   PT_LOAD FLAGS(5);    /* R X */
    data   PT_LOAD FLAGS(6);    /* R W */
    note   PT_NOTE FLAGS(4);    /* what the kernel reads of a handover record */
}

SECTIONS
{
    . = 0xffffffff80000000;
    __kernel_start = .;

    __rodata_start = .;
    /* Written by the bootloader before entry, read-only afterwards. */
    .limine_requests : {
        KEEP(*(.limine_requests_start))
        KEEP(*(.limine_requests))
        KEEP(*(.limine_requests_end))
    } :rodata
    .rodata : { *(.rodata .rodata.*) } :rodata
    /* handover::version's note (R9 (d)): in the read-only segment and in a
       PT_NOTE of its own, so a running kernel finds it in the next one's file. */
    .gala_note : ALIGN(8) { KEEP(*(.gala_note)) } :rodata :note
    . = ALIGN(4096);
    __rodata_end = .;

    __text_start = .;
    .text : { *(.text .text.*) } :text
    . = ALIGN(4096);
    __text_end = .;

    __data_start = .;
    .data : { *(.data .data.*) } :data
    .bss : {
        *(.bss .bss.*)
        *(COMMON)
    } :data
    . = ALIGN(4096);
    __data_end = .;

    __kernel_end = .;

    /DISCARD/ : { *(.eh_frame*) *(.note .note.*) }
}
