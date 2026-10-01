OUTPUT_FORMAT(elf64-x86-64)
ENTRY(_start)

/* The real kernel is linked at 0xffffffff80000000 and unpacked there at boot. This stub reserves
   that range as zeroed memory (a segment with no file contents) so Limine maps it, and lives
   right above it (Limine loads the whole span between the lowest and highest segment as one block). PAYLOAD_SIZE must match kstub's PAYLOAD_SIZE and xtask's check. */
PAYLOAD_SIZE = 4M;

PHDRS
{
    payload PT_LOAD FLAGS((1 << 0) | (1 << 1) | (1 << 2)); /* X W R */
    text    PT_LOAD FLAGS((1 << 0) | (1 << 2));
    rodata  PT_LOAD FLAGS((1 << 2));
    data    PT_LOAD FLAGS((1 << 1) | (1 << 2));
}

SECTIONS
{
    . = 0xffffffff80000000;
    .payload (NOLOAD) : { . += PAYLOAD_SIZE; } :payload

    . = 0xffffffff80000000 + PAYLOAD_SIZE;
    .text : { *(.text .text.*) } :text

    . = ALIGN(4K);
    .rodata : { *(.rodata .rodata.*) } :rodata

    . = ALIGN(4K);
    .data : {
        *(.data .data.*)
        KEEP(*(.limine_requests_start))
        __req_start = .;
        KEEP(*(.limine_requests))
        __req_end = .;
        KEEP(*(.limine_requests_end))
    } :data

    .bss : { *(.bss .bss.*) *(COMMON) } :data

    /DISCARD/ : { *(.eh_frame*) *(.note .note.*) *(.comment) }
}
