OUTPUT_FORMAT(elf64-x86-64)
ENTRY(s1_start)

SECTIONS
{
    . = 0x7c00;
    .text : { *(.s1) }
    /DISCARD/ : { *(.text .text.* .data .data.* .rodata .rodata.* .bss .bss.* .eh_frame* .note* .comment) }
}
