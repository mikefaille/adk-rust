//! Audio DSP configuration to guard against CPU saturation.
//!
//! When bridging real-time audio components, silent packets or near-silent noise
//! can generate subnormal floating-point numbers. On x86 processors, these fall back
//! to legacy microcode emulation, causing extreme CPU lockups.

/// Configures the CPU hardware registers to utilize Flush-to-Zero (FTZ) and
/// Denormals-Are-Zero (DAZ) states.
///
/// This must be called at the startup thread entry point or before audio streaming
/// begins to prevent deep microcode stalls during processing.
#[inline(always)]
pub fn enable_hardware_dsp_optimizations() {
    #[cfg(target_arch = "x86_64")]
    unsafe {
        let mut mxcsr: u32 = 0;
        std::arch::asm!(
            "stmxcsr [{0}]",
            in(reg) &mut mxcsr,
        );
        // Bit 15 is FTZ (Flush-To-Zero), Bit 6 is DAZ (Denormals-Are-Zero).
        mxcsr |= (1 << 15) | (1 << 6);
        std::arch::asm!(
            "ldmxcsr [{0}]",
            in(reg) &mxcsr,
        );
    }
}
