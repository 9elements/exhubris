#![no_std]
#![no_main]

mod startup;

use device::rcc::vals::*;
use stm32_metapac::{
    self as device,
    // flash::vals::Latency,
    // rcc::vals::{Pllm, Plln, Pllp, Pllq, Pllr, Pllsrc, Sw},
};

use cortex_m_rt::entry;

#[entry]
fn main() -> ! {
    const CYCLES_PER_MS: u32 = 400_000;
    const CLOCKS: startup::ClockConfig = startup::ClockConfig {
        // The Nucleo board doesn't include an external crystal, so we
        // derive clocks from the HSI64 oscillator.
        source: startup::ClockSource::Hsi64,
        // 64MHz oscillator frequency is outside VCO input range of
        // 2-16, so we use DIVM to divide it by 4 to 16MHz.
        // divm: 4,
        divm: Pllm::DIV4,
        // This means the VCO must accept its wider input range:
        vcosel: Pllvcosel::WIDE_VCO,
        pllrange: Pllrge::RANGE8,
        // DIVN governs the multiplication of the VCO input frequency to
        // produce the intermediate frequency. We want an IF of 800MHz,
        // or a multiplication of 50x.
        //
        // We subtract 1 to get the DIVN value because the PLL
        // effectively adds one to what we write.
        // divn: 50 - 1,
        divn: Plln::MUL50,
        // P is the divisor from the VCO IF to the system frequency. We
        // want 400MHz, so:
        divp: Plldiv::DIV2,
        // Q produces kernel clocks; we set it to 200MHz:
        // divq: 4 - 1,
        divq: Plldiv::DIV4,
        // R is mostly used by the trace unit and we leave it fast:
        // divr: 2 - 1,
        divr: Plldiv::DIV2,

        // We run the CPU at the full core rate of 400MHz:
        cpu_div: Hpre::DIV1,
        // We down-shift the AHB by a factor of 2, to 200MHz, to meet
        // its constraints (Table 122 in datasheet)
        ahb_div: Hpre::DIV2,
        // We configure all APB for 100MHz. These are relative to the
        // AHB frequency.
        apb1_div: Ppre::DIV2,
        apb2_div: Ppre::DIV2,
        apb3_div: Ppre::DIV2,
        apb4_div: Ppre::DIV2,

        // Flash runs at 200MHz: 2WS, 2 programming cycles. See
        // reference manual Table 13.
        flash_latency: 2,
        flash_write_delay: 2,
    };

    startup::system_init(CLOCKS);
    unsafe { hubris_kern::startup::start_kernel(CYCLES_PER_MS) }
}
