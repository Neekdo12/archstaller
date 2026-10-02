    .section .s2entry,"ax"
    .code16
    .global s2_start
s2_start:
    jmp s2_real
    .org 0x20
    .global s2_header
s2_header:
    .space 568

# ---------------------------------------------------------------------------------------------
# Real mode: A20, memory map, graphics mode, read what to boot into high memory.
# Low memory map: 0x500 handoff, 0x600 E820 table, 0x1200 VBE info, 0x1400 mode info, 0x1500 EDID,
# 0x1600 mode list, stack below 0x7000, 0x60000 bounce buffer for disk reads (the images 0x8000+).
# ---------------------------------------------------------------------------------------------
s2_real:
    cli
    cld
    xor ax, ax
    mov ds, ax
    mov es, ax
    mov ss, ax
    mov sp, 0x7000
    mov byte ptr [drive], dl
    movzx ebx, bx
    mov dword ptr [sector_size], ebx
    sti
    mov si, offset msg_start
    call print

    call a20_enable
    call e820_read
    call vbe_init
    call read_items
    jmp enter_long

# --- output: BIOS teletype and COM1 -----------------------------------------------------------
print:                                  # ds:si = NUL terminated string
    pusha
1:
    lodsb
    test al, al
    jz 3f
    mov ah, 0x0e
    xor bx, bx
    int 0x10
    mov dx, 0x3f8
    out dx, al
    jmp 1b
3:
    popa
    ret

fail:                                   # si = message
    call print
    cli
1:
    hlt
    jmp 1b

# --- A20 (the order is Limine's: BIOS call, keyboard controller, fast A20) ------------------------
a20_check:                              # returns ZF clear when A20 is on
    push ds
    push es
    push si
    push di
    xor ax, ax
    mov ds, ax
    not ax
    mov es, ax                          # es = 0xffff: es:0x7e0e is 0x107dfe, which wraps to 0x7dfe with A20 off
    mov si, 0x7dfe
    mov di, 0x7e0e
    mov ax, word ptr [si]
    push ax
    mov word ptr [si], 0x1234
    cmp word ptr es:[di], 0x1234
    jne 1f                              # differs: A20 on
    not word ptr [si]
    mov ax, word ptr [si]
    cmp word ptr es:[di], ax
    jne 1f
    pop ax
    mov word ptr [si], ax
    xor ax, ax                          # ZF set: off
    jmp 2f
1:
    pop ax
    mov word ptr [si], ax
    or ax, 1                            # ZF clear: on
2:
    pop di
    pop si
    pop es
    pop ds
    ret

kbc_wait_in:                            # wait until the controller's input buffer is empty (bounded)
    mov cx, 0xffff
1:
    in al, 0x64
    test al, 2
    jz 2f
    loop 1b
    stc
    ret
2:
    clc
    ret

kbc_wait_out:                           # wait until there is output to read (bounded)
    mov cx, 0xffff
1:
    in al, 0x64
    test al, 1
    jnz 2f
    loop 1b
    stc
    ret
2:
    clc
    ret

a20_enable:
    call a20_check
    jnz 9f
    mov ax, 0x2401                      # BIOS
    int 0x15
    call a20_check
    jnz 9f
    call kbc_wait_in                    # keyboard controller (a machine without one just times out)
    jc 5f
    mov al, 0xad
    out 0x64, al
    call kbc_wait_in
    jc 5f
    mov al, 0xd0
    out 0x64, al
    call kbc_wait_out
    jc 5f
    in al, 0x60
    push ax
    call kbc_wait_in
    jc 4f
    mov al, 0xd1
    out 0x64, al
    call kbc_wait_in
    jc 4f
    pop ax
    or al, 2
    out 0x60, al
    call kbc_wait_in
    mov al, 0xae
    out 0x64, al
    call kbc_wait_in
    jmp 6f
4:
    pop ax
5:
6:
    call a20_check
    jnz 9f
    in al, 0x92                         # fast A20
    test al, 2
    jnz 7f
    and al, 0xfe
    or al, 2
    out 0x92, al
7:
    mov cx, 0xffff                      # the line takes a moment
8:
    call a20_check
    jnz 9f
    loop 8b
    mov si, offset msg_a20
    jmp fail
9:
    ret

# --- E820 ----------------------------------------------------------------------------------------
e820_read:
    xor ebx, ebx
    mov di, 0x600
1:
    mov dword ptr [di + 20], 1          # extended attributes: valid, in case the BIOS only writes 20 bytes
    mov eax, 0xe820
    mov edx, 0x534d4150
    mov ecx, 24
    int 0x15
    jc 3f
    cmp eax, 0x534d4150
    jne 8f
    inc word ptr [e820_count]
    add di, 24
    cmp word ptr [e820_count], 120
    jae 4f
    test ebx, ebx
    jnz 1b
    jmp 4f
3:
    cmp word ptr [e820_count], 0
    jne 4f
    # No E820: INT 15h E801 (memory above 1 MiB in two pieces), else the oldest call (88h).
    mov ax, 0xe801
    int 0x15
    jc 6f
    test ax, ax
    jnz 5f
    mov ax, cx
    mov bx, dx
5:
    movzx eax, ax                       # KiB from 1 MiB to 16 MiB
    shl eax, 10
    movzx ebx, bx                       # 64 KiB blocks above 16 MiB
    shl ebx, 16
    mov di, 0x600
    mov dword ptr [di], 0x100000
    mov dword ptr [di + 4], 0
    mov dword ptr [di + 8], eax
    mov dword ptr [di + 12], 0
    mov dword ptr [di + 16], 1
    mov dword ptr [di + 20], 1
    mov word ptr [e820_count], 1
    test ebx, ebx
    jz 4f
    mov dword ptr [di + 24], 0x1000000
    mov dword ptr [di + 28], 0
    mov dword ptr [di + 32], ebx
    mov dword ptr [di + 36], 0
    mov dword ptr [di + 40], 1
    mov dword ptr [di + 44], 1
    mov word ptr [e820_count], 2
    jmp 4f
6:
    mov ah, 0x88
    int 0x15
    jc 7f
    movzx eax, ax
    shl eax, 10
    mov di, 0x600
    mov dword ptr [di], 0x100000
    mov dword ptr [di + 4], 0
    mov dword ptr [di + 8], eax
    mov dword ptr [di + 12], 0
    mov dword ptr [di + 16], 1
    mov dword ptr [di + 20], 1
    mov word ptr [e820_count], 1
    jmp 4f
7:
    mov si, offset msg_mem
    jmp fail
8:
    mov si, offset msg_e820
    jmp fail
4:
    movzx eax, word ptr [e820_count]
    mov dword ptr [0x500], eax          # handoff: entry count
    ret

# --- VBE ---------------------------------------------------------------------------------------------
# Picks a 32 bpp direct colour linear framebuffer mode: the monitor's preferred size (EDID) when the
# card offers it, else the largest up to 1920x1200, else the smallest. No mode is not an error.
vbe_init:
    pusha
    mov dword ptr [0x510], 0            # handoff: framebuffer address (0: none)
    mov dword ptr [want_wh], 0
    mov ax, 0x4f15                      # EDID of the first display
    mov bl, 1
    xor cx, cx
    xor dx, dx
    mov di, 0x1500
    int 0x10
    cmp ax, 0x004f
    jne 1f
    mov ax, word ptr [0x1536]           # first detailed timing: pixel clock (0 = not a timing)
    test ax, ax
    jz 1f
    movzx eax, byte ptr [0x1538]
    movzx ecx, byte ptr [0x153a]
    shr ecx, 4
    shl ecx, 8
    or eax, ecx
    mov word ptr [want_w], ax
    movzx eax, byte ptr [0x153b]
    movzx ecx, byte ptr [0x153d]
    shr ecx, 4
    shl ecx, 8
    or eax, ecx
    mov word ptr [want_h], ax
1:
    mov dword ptr [0x1200], 0x32454256   # "VBE2": ask for the VBE 2 layout
    mov ax, 0x4f00
    mov di, 0x1200
    int 0x10
    cmp ax, 0x004f
    jne 9f
    cmp dword ptr [0x1200], 0x41534556   # "VESA"
    jne 9f
    cmp word ptr [0x1204], 0x0200
    jb 9f
    # Copy the mode list: it may point into the info block, which later calls overwrite.
    mov si, word ptr [0x120e]
    mov ax, word ptr [0x1210]
    mov fs, ax
    mov di, 0x1600
    mov cx, 200
2:
    mov ax, word ptr fs:[si]
    mov word ptr [di], ax
    add si, 2
    add di, 2
    cmp ax, 0xffff
    je 3f
    loop 2b
    mov word ptr [di], 0xffff
3:
    xor ax, ax
    mov fs, ax
    mov word ptr [best_mode], 0
    mov dword ptr [best_score], 0
    mov si, 0x1600
4:
    mov cx, word ptr [si]
    cmp cx, 0xffff
    je 8f
    add si, 2
    push si
    mov ax, 0x4f01
    mov di, 0x1400
    int 0x10
    pop si
    cmp ax, 0x004f
    jne 4b
    mov ax, word ptr [0x1400]
    and ax, 0x99                        # supported, colour, graphics, linear framebuffer
    cmp ax, 0x99
    jne 4b
    cmp byte ptr [0x1419], 32
    jne 4b
    cmp byte ptr [0x141b], 6            # direct colour
    jne 4b
    movzx eax, word ptr [0x1412]        # width
    movzx edx, word ptr [0x1414]        # height
    cmp ax, word ptr [want_w]
    jne 5f
    cmp dx, word ptr [want_h]
    jne 5f
    mov word ptr [best_mode], cx        # exactly what the monitor wants
    jmp 8f
5:
    mov ebx, eax
    imul ebx, edx                       # pixels
    cmp eax, 1920
    ja 6f
    cmp edx, 1200
    ja 6f
    jmp 7f                              # fits: more pixels is better
6:
    mov eax, 0x20000000                 # too big: only wanted when nothing fits, and then the smallest
    xor edx, edx
    div ebx
    mov ebx, eax
7:
    cmp ebx, dword ptr [best_score]
    jbe 4b
    mov dword ptr [best_score], ebx
    mov word ptr [best_mode], cx
    jmp 4b
8:
    mov cx, word ptr [best_mode]
    test cx, cx
    jz 9f
    mov bx, cx
    or bx, 0x4000                       # linear framebuffer
    mov ax, 0x4f02
    int 0x10
    cmp ax, 0x004f
    jne 9f
    mov ax, 0x4f01                      # the mode as set (cx was preserved by the BIOS call or not: reload it)
    mov cx, word ptr [best_mode]
    mov di, 0x1400
    int 0x10
    cmp ax, 0x004f
    jne 9f
    mov eax, dword ptr [0x1428]
    mov dword ptr [0x510], eax          # handoff: address
    movzx eax, word ptr [0x1412]
    mov dword ptr [0x514], eax          # width
    movzx eax, word ptr [0x1414]
    mov dword ptr [0x518], eax          # height
    movzx eax, word ptr [0x1410]
    cmp word ptr [0x1204], 0x0300
    jb 10f
    movzx edx, word ptr [0x1432]        # VBE 3: linear bytes per scan line
    test edx, edx
    jz 10f
    mov eax, edx
10:
    mov dword ptr [0x51c], eax          # pitch
    mov al, byte ptr [0x1420]
    mov byte ptr [0x520], al            # red shift
    mov al, byte ptr [0x1422]
    mov byte ptr [0x521], al            # green shift
    mov al, byte ptr [0x1424]
    mov byte ptr [0x522], al            # blue shift
9:
    xor ax, ax
    mov ds, ax
    mov es, ax
    popa
    ret

# --- Reading into high memory -----------------------------------------------------------------------------
# The header's three extents are read to consecutive 1 MiB aligned addresses from 16 MiB; handoff
# 0x540.. gets (address, length) for each as two dwords each.
read_items:
    pusha
    mov dword ptr [dest], 0x1000000
    xor di, di                          # item number
1:
    cmp di, 3
    jae 9f
    mov bx, di
    shl bx, 4
    mov eax, dword ptr [s2_header + 8 + bx]       # offset (low dword)
    mov edx, dword ptr [s2_header + 12 + bx]
    mov ecx, dword ptr [s2_header + 16 + bx]      # length (low dword)
    mov esi, dword ptr [s2_header + 20 + bx]
    or edx, esi
    jnz 8f                                          # offsets and sizes above 4 GiB are not supported
    mov ebx, dword ptr [dest]
    mov dword ptr [item_off], eax
    mov dword ptr [item_len], ecx
    # handoff entry
    movzx eax, di
    shl ax, 3
    mov si, ax
    mov dword ptr [0x540 + si], ebx
    mov dword ptr [0x544 + si], ecx
    test ecx, ecx
    jz 2f
    call read_one
    mov eax, dword ptr [dest]
    add eax, dword ptr [item_len]
    add eax, 0xfffff
    and eax, 0xfff00000
    mov dword ptr [dest], eax
2:
    inc di
    jmp 1b
8:
    mov si, offset msg_big
    jmp fail
9:
    popa
    ret

# Reads item_len bytes from byte offset item_off of the boot medium to the linear address [dest].
read_one:
    pushad
    mov eax, dword ptr [item_off]
    xor edx, edx
    div dword ptr [sector_size]
    mov dword ptr [rd_lba], eax
    mov dword ptr [rd_skip], edx
    mov eax, dword ptr [item_len]
    mov dword ptr [rd_left], eax
    mov eax, dword ptr [dest]
    mov dword ptr [rd_dest], eax
1:
    cmp dword ptr [rd_left], 0
    je 9f
    # sectors to read: enough for skip + left, at most 64 KiB worth
    mov eax, dword ptr [rd_skip]
    add eax, dword ptr [rd_left]
    add eax, dword ptr [sector_size]
    dec eax
    xor edx, edx
    div dword ptr [sector_size]
    mov ecx, eax
    mov eax, 0x8000                     # at most 32 KiB per call (some BIOSes refuse more than 127 sectors)
    xor edx, edx
    div dword ptr [sector_size]
    cmp ecx, eax
    jbe 2f
    mov ecx, eax
2:
    mov word ptr [rd_dap + 2], cx
    mov eax, dword ptr [rd_lba]
    mov dword ptr [rd_dap + 8], eax
    mov word ptr [rd_dap + 6], 0x6000     # bounce buffer 0x6000:0000 = 0x60000
    mov word ptr [rd_dap + 4], 0
    mov byte ptr [rd_tries], 3
3:
    mov si, offset rd_dap
    mov ah, 0x42
    mov dl, byte ptr [drive]
    int 0x13
    jnc 5f
    xor ax, ax
    mov dl, byte ptr [drive]
    int 0x13
    dec byte ptr [rd_tries]
    jnz 3b
    mov si, offset msg_disk
    jmp fail
5:
    # bytes available = sectors * sector size - skip, take min(that, left)
    movzx eax, word ptr [rd_dap + 2]
    mul dword ptr [sector_size]
    sub eax, dword ptr [rd_skip]
    cmp eax, dword ptr [rd_left]
    jbe 6f
    mov eax, dword ptr [rd_left]
6:
    mov dword ptr [rd_take], eax
    mov esi, 0x60000
    add esi, dword ptr [rd_skip]
    mov edi, dword ptr [rd_dest]
    mov ecx, eax
    call copy_high
    mov eax, dword ptr [rd_take]
    add dword ptr [rd_dest], eax
    sub dword ptr [rd_left], eax
    movzx eax, word ptr [rd_dap + 2]
    add dword ptr [rd_lba], eax
    mov dword ptr [rd_skip], 0
    jmp 1b
9:
    popad
    ret

# esi = source, edi = destination (linear), ecx = bytes. Goes through 32-bit protected mode (flat) and back.
copy_high:
    cli
    lgdt [gdtr]
    mov eax, cr0
    or al, 1
    mov cr0, eax
    .byte 0x66, 0xea
    .long 2f
    .word 0x08
    .code32
2:
    mov ax, 0x10
    mov ds, ax
    mov es, ax
    mov ss, ax
    cld
    rep movsb
    .byte 0xea
    .long 3f
    .word 0x18
    .code16
3:
    mov ax, 0x20
    mov ds, ax
    mov es, ax
    mov ss, ax
    mov eax, cr0
    and al, 0xfe
    mov cr0, eax
    .byte 0xea
    .word 4f
    .word 0
4:
    xor ax, ax
    mov ds, ax
    mov es, ax
    mov ss, ax
    sti
    ret

# --- Into long mode ------------------------------------------------------------------------------------------
enter_long:
    cli
    lgdt [gdtr]
    mov eax, cr0
    or al, 1
    mov cr0, eax
    .byte 0x66, 0xea
    .long 2f
    .word 0x08
    .code32
2:
    mov ax, 0x10
    mov ds, ax
    mov es, ax
    mov ss, ax
    mov fs, ax
    mov gs, ax
    mov esp, 0x7000
    # a 64-bit CPU?
    mov eax, 0x80000000
    cpuid
    cmp eax, 0x80000001
    jb 8f
    mov eax, 0x80000001
    cpuid
    test edx, 1 << 29
    jz 8f
    # clear .bss (the page tables are in it)
    mov edi, offset __bss_start
    mov ecx, offset __bss_end
    sub ecx, edi
    xor eax, eax
    rep stosb
    # identity map the first 4 GiB with 2 MiB pages: PML4 -> PDPT -> four page directories
    mov edi, offset boot_pt
    lea eax, [edi + 0x1000 + 3]
    mov dword ptr [edi], eax
    lea edx, [edi + 0x1000]
    lea eax, [edi + 0x2000 + 3]
    mov ecx, 4
3:
    mov dword ptr [edx], eax
    add edx, 8
    add eax, 0x1000
    loop 3b
    lea edx, [edi + 0x2000]
    mov eax, 0x83
    mov ecx, 2048
4:
    mov dword ptr [edx], eax
    mov dword ptr [edx + 4], 0
    add edx, 8
    add eax, 0x200000
    loop 4b
    mov eax, cr4
    or eax, 1 << 5                      # PAE
    mov cr4, eax
    mov eax, offset boot_pt
    mov cr3, eax
    mov ecx, 0xc0000080                 # EFER.LME
    rdmsr
    or eax, 1 << 8
    wrmsr
    mov eax, cr0
    or eax, 1 << 31                     # paging
    mov cr0, eax
    .byte 0xea
    .long 5f
    .word 0x28
8:
    mov esi, offset msg_cpu
9:
    mov dx, 0x3f8                        # no BIOS here: serial and VGA text
    mov edi, 0xb8000
10:
    lodsb
    test al, al
    jz 11f
    out dx, al
    mov ah, 0x4f
    stosw
    jmp 10b
11:
    hlt
    jmp 11b
    .code64
5:
    mov ax, 0x10
    mov ds, ax
    mov es, ax
    mov ss, ax
    mov rsp, 0x80000
    xor ebp, ebp
    call bios_main
12:
    hlt
    jmp 12b

    .balign 8
gdt:
    .quad 0
    .quad 0x00cf9a000000ffff            # 0x08 code32
    .quad 0x00cf92000000ffff            # 0x10 data32
    .quad 0x00009a000000ffff            # 0x18 code16
    .quad 0x000092000000ffff            # 0x20 data16
    .quad 0x00af9a000000ffff            # 0x28 code64
gdtr:
    .word 6 * 8 - 1
    .long gdt
    .long 0

    .balign 4
drive:       .long 0
sector_size: .long 512
e820_count:  .long 0
want_wh:
want_w:      .word 0
want_h:      .word 0
best_mode:   .word 0
best_score:  .long 0
dest:        .long 0
item_off:    .long 0
item_len:    .long 0
rd_lba:      .long 0
rd_skip:     .long 0
rd_left:     .long 0
rd_dest:     .long 0
rd_take:     .long 0
rd_tries:    .byte 0
    .balign 4
rd_dap:
    .byte 0x10, 0
    .word 0
    .word 0
    .word 0
    .quad 0
msg_start:   .asciz "boot: BIOS loader\r\n"
msg_a20:     .asciz "boot: cannot enable the A20 line\r\n"
msg_mem:     .asciz "boot: the BIOS reports no memory map\r\n"
msg_e820:    .asciz "boot: the BIOS memory map is unreliable\r\n"
msg_disk:    .asciz "boot: disk read error\r\n"
msg_big:     .asciz "boot: image beyond 4 GiB on the boot medium\r\n"
msg_cpu:     .asciz "boot: this CPU is not 64-bit\r\n"
