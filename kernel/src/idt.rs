//! Own GDT + TSS (for the double-fault IST stack) and an IDT that only handles CPU exceptions.
use core::arch::asm;
use core::mem::size_of;

#[repr(C)]
struct InterruptStackFrame {
    rip: u64,
    cs: u64,
    rflags: u64,
    rsp: u64,
    ss: u64,
}

#[repr(C, packed(4))]
struct Tss {
    _r0: u32,
    rsp: [u64; 3],
    _r1: u64,
    ist: [u64; 7],
    _r2: u64,
    _r3: u16,
    iomap_base: u16,
}

#[repr(C, align(16))]
struct Stack([u8; 16 * 1024]);

static mut DF_STACK: Stack = Stack([0; 16 * 1024]);
static mut TSS: Tss = Tss {
    _r0: 0,
    rsp: [0; 3],
    _r1: 0,
    ist: [0; 7],
    _r2: 0,
    _r3: 0,
    iomap_base: size_of::<Tss>() as u16,
};
// null, kernel code, kernel data, TSS (two slots)
static mut GDT: [u64; 5] = [0; 5];

#[repr(C, packed)]
struct DescTablePtr {
    limit: u16,
    base: u64,
}

#[derive(Clone, Copy)]
#[repr(C)]
struct Gate {
    offset_lo: u16,
    selector: u16,
    ist: u8,
    attrs: u8,
    offset_mid: u16,
    offset_hi: u32,
    _zero: u32,
}

impl Gate {
    const MISSING: Gate = Gate { offset_lo: 0, selector: 0, ist: 0, attrs: 0, offset_mid: 0, offset_hi: 0, _zero: 0 };

    fn new(handler: u64, ist: u8) -> Gate {
        Gate {
            offset_lo: handler as u16,
            selector: 0x08,
            ist,
            attrs: 0x8e, // present, interrupt gate
            offset_mid: (handler >> 16) as u16,
            offset_hi: (handler >> 32) as u32,
            _zero: 0,
        }
    }
}

static mut IDT: [Gate; 32] = [Gate::MISSING; 32];

fn report(name: &str, vector: u8, code: Option<u64>, f: &InterruptStackFrame) -> ! {
    unsafe { crate::console::force_unlock() };
    crate::println!("\n*** EXCEPTION #{vector} {name}");
    if let Some(c) = code {
        crate::println!("error code: {c:#x}");
    }
    if vector == 14 {
        let cr2: u64;
        unsafe { asm!("mov {}, cr2", out(reg) cr2, options(nomem, nostack)) };
        crate::println!("cr2: {cr2:#x}");
    }
    crate::println!("rip: {:#x}  cs: {:#x}  rflags: {:#x}", f.rip, f.cs, f.rflags);
    crate::println!("rsp: {:#x}  ss: {:#x}", f.rsp, f.ss);
    crate::halt()
}

macro_rules! handler {
    ($name:ident, $vec:expr, $label:expr) => {
        extern "x86-interrupt" fn $name(f: InterruptStackFrame) {
            report($label, $vec, None, &f)
        }
    };
    ($name:ident, $vec:expr, $label:expr, err) => {
        extern "x86-interrupt" fn $name(f: InterruptStackFrame, code: u64) {
            report($label, $vec, Some(code), &f)
        }
    };
}

handler!(divide_error, 0, "divide error");
handler!(debug, 1, "debug");
handler!(nmi, 2, "NMI");
handler!(breakpoint, 3, "breakpoint");
handler!(overflow, 4, "overflow");
handler!(bound_range, 5, "bound range exceeded");
handler!(invalid_opcode, 6, "invalid opcode");
handler!(device_not_available, 7, "device not available");
handler!(double_fault, 8, "double fault", err);
handler!(invalid_tss, 10, "invalid TSS", err);
handler!(segment_not_present, 11, "segment not present", err);
handler!(stack_segment, 12, "stack segment fault", err);
handler!(general_protection, 13, "general protection fault", err);
handler!(page_fault, 14, "page fault", err);
handler!(x87_fp, 16, "x87 floating point");
handler!(alignment_check, 17, "alignment check", err);
handler!(machine_check, 18, "machine check");
handler!(simd_fp, 19, "SIMD floating point");

pub fn init() {
    unsafe {
        let gdt = &raw mut GDT;
        let tss = &raw mut TSS;
        (*tss).ist[0] = (&raw const DF_STACK as u64) + size_of::<Stack>() as u64;

        let tss_base = tss as u64;
        let tss_limit = (size_of::<Tss>() - 1) as u64;
        (*gdt)[1] = 0x00af_9a00_0000_ffff; // 64-bit code
        (*gdt)[2] = 0x00cf_9200_0000_ffff; // data
        (*gdt)[3] = (tss_limit & 0xffff)
            | ((tss_base & 0xff_ffff) << 16)
            | (0x89 << 40) // present, available 64-bit TSS
            | (((tss_limit >> 16) & 0xf) << 48)
            | (((tss_base >> 24) & 0xff) << 56);
        (*gdt)[4] = tss_base >> 32;

        let gdtr = DescTablePtr { limit: (size_of::<[u64; 5]>() - 1) as u16, base: gdt as u64 };
        asm!(
            "lgdt [{gdtr}]",
            "push 0x08",
            "lea {tmp}, [rip + 2f]",
            "push {tmp}",
            "retfq",
            "2:",
            "mov ax, 0x10",
            "mov ds, ax",
            "mov es, ax",
            "mov ss, ax",
            "xor eax, eax",
            "mov fs, ax",
            "mov gs, ax",
            "mov ax, 0x18",
            "ltr ax",
            gdtr = in(reg) &gdtr,
            tmp = out(reg) _,
            out("rax") _,
        );

        let idt = &raw mut IDT;
        let set = |v: usize, h: u64, ist: u8| (*idt)[v] = Gate::new(h, ist);
        set(0, divide_error as *const () as u64, 0);
        set(1, debug as *const () as u64, 0);
        set(2, nmi as *const () as u64, 0);
        set(3, breakpoint as *const () as u64, 0);
        set(4, overflow as *const () as u64, 0);
        set(5, bound_range as *const () as u64, 0);
        set(6, invalid_opcode as *const () as u64, 0);
        set(7, device_not_available as *const () as u64, 0);
        set(8, double_fault as *const () as u64, 1);
        set(10, invalid_tss as *const () as u64, 0);
        set(11, segment_not_present as *const () as u64, 0);
        set(12, stack_segment as *const () as u64, 0);
        set(13, general_protection as *const () as u64, 0);
        set(14, page_fault as *const () as u64, 0);
        set(16, x87_fp as *const () as u64, 0);
        set(17, alignment_check as *const () as u64, 0);
        set(18, machine_check as *const () as u64, 0);
        set(19, simd_fp as *const () as u64, 0);

        let idtr = DescTablePtr { limit: (size_of::<[Gate; 32]>() - 1) as u16, base: idt as u64 };
        asm!("lidt [{}]", in(reg) &idtr, options(readonly, nostack));
    }
}
