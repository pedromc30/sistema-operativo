OUTPUT_FORMAT(elf64-x86-64)
ENTRY(_start)

PHDRS
{
    text   PT_LOAD FLAGS(5);  /* R+X */
    rodata PT_LOAD FLAGS(4);  /* R   */
    data   PT_LOAD FLAGS(6);  /* R+W */
}

SECTIONS
{
    . = 0xffffffff80000000;

    .text : { *(.text .text.*) } :text

    . = ALIGN(4K);
    .rodata : { *(.rodata .rodata.*) } :rodata

    . = ALIGN(4K);
    .data : {
        *(.data .data.*)
        KEEP(*(.requests))
    } :data
    .bss : { *(.bss .bss.*) *(COMMON) } :data

    /DISCARD/ : { *(.eh_frame*) *(.note*) *(.comment) }
}
