.section ".text._start", "ax"
.global _start

_start:
    // Only allow Core 0 to run; park secondary cores in wfe
    mrs     x1, mpidr_el1
    and     x1, x1, #0xff
    cbz     x1, 1f
2:
    wfe
    b       2b

1:
    // Check current Exception Level (CurrentEL[3:2])
    mrs     x1, CurrentEL
    lsr     x1, x1, #2
    cmp     x1, #3
    b.eq    el3_entry
    cmp     x1, #2
    b.eq    el2_entry
    b       el1_entry

el3_entry:
    // Set up SCR_EL3: NS (bit 0) = 1 (Non-secure), SMD (bit 7) = 1, RW (bit 10) = 1 (EL2 is AArch64)
    mov     x1, #(1 | (1 << 10))
    msr     scr_el3, x1
    // SPSR_EL3 for EL2h (0b1001, DAIF masked = 0x3c9)
    mov     x1, #0x3c9
    msr     spsr_el3, x1
    adr     x1, el2_entry
    msr     elr_el3, x1
    eret

el2_entry:
    // Allow EL1 to access physical timer
    mrs     x1, cnthctl_el2
    orr     x1, x1, #3
    msr     cnthctl_el2, x1
    msr     cntvoff_el2, xzr

    // Enable AArch64 in EL1 (HCR_EL2.RW = 1)
    mov     x1, #(1 << 31)
    msr     hcr_el2, x1

    // Setup SPSR_EL2 for EL1h (M[3:0] = 0b0101, DAIF masked = 0x3c0)
    mov     x1, #0x3c5
    msr     spsr_el2, x1

    // Return to el1_entry
    adr     x1, el1_entry
    msr     elr_el2, x1
    eret

el1_entry:
    // Set up stack pointer
    ldr     x1, =__stack_top
    mov     sp, x1

    // Enable floating point and NEON (CPACR_EL1.FPEN = 0b11)
    mov     x1, #(3 << 20)
    msr     cpacr_el1, x1
    isb

    // Zero out BSS section
    ldr     x1, =__bss_start
    ldr     x2, =__bss_end
3:
    cmp     x1, x2
    b.ge    4f
    str     xzr, [x1], #8
    b       3b

4:
    // Jump to Rust kernel main (DTB pointer remains in x0)
    b       kmain

// Safety fallback if kmain returns
halt:
    wfe
    b       halt
