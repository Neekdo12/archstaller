OUTPUT_FORMAT(elf64-x86-64)
ENTRY(s2_start)

SECTIONS
{
    . = 0x8000;
    .text : { KEEP(*(.s2entry)) *(.text .text.*) }
    .rodata : { *(.rodata .rodata.*) }
    .data : { *(.data .data.*) }
    . = ALIGN(4096);
    __bss_start = .;
    .bss : { *(.bss .bss.*) *(COMMON) }
    . = ALIGN(8);
    __bss_end = .;
    /DISCARD/ : { *(.eh_frame*) *(.note .note.*) *(.comment) }
}
