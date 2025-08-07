// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

#![no_std]

use cortex_m_rt::pre_init;
use stm32_metapac as device;

#[pre_init]
unsafe fn system_pre_init() {
    // Configure the power supply to latch the LDO on and prevent further
    // reconfiguration.
    //
    // Normally we would use Peripherals::take() to safely get a reference to
    // the PWR block, but that function expects RAM to be initialized and
    // writable. At this point, RAM is neither -- because the chip requires us
    // to get the power supply configuration right _before it guarantees that
    // RAM will work._
    //
    // Another case of the cortex_m/stm32 crates being designed with simpler
    // systems in mind.

    // Synthesize a pointer using a const fn (which won't hit RAM) and then
    // convert it to a reference. We can have a reference to PWR because it's
    // hardware, and is thus not uninitialized.
    // let pwr = device::PWR.as_ptr();
    // Poke CR3 to enable the LDO and prevent further writes.
    // pwr.cr3().modify(|_, w| w.ldoen().set_bit());
    device::PWR.cr3().modify(|w| w.set_ldoen(true));

    // Busy-wait until the ACTVOSRDY bit says that we've stabilized at VOS3.
    // while !pwr.csr1.read().actvosrdy().bit() {
    //     // spin
    // }
    while !device::PWR.csr1().read().actvosrdy() {
        //spin
    }

    // Turn on the internal RAMs.
    // let rcc = device::RCC.as_ptr();
    // rcc.ahb2enr.modify(|_, w| {
    //     w.sram1en()
    //         .set_bit()
    //         .sram2en()
    //         .set_bit()
    //         .sram3en()
    //         .set_bit()
    // });
    device::RCC.ahb2enr().modify(|w| {
        w.set_sram1en(true);
        w.set_sram2en(true);
        w.set_sram3en(true);
    })

    // Okay, yay, we can use some RAMs now.

    // We'll do the rest in system_init.
}

pub struct ClockConfig {
    pub source: ClockSource,
    pub divm: device::rcc::vals::Pllm,
    pub vcosel: device::rcc::vals::Pllvcosel,
    pub pllrange: device::rcc::vals::Pllrge,
    pub divn: device::rcc::vals::Plln,
    pub divp: device::rcc::vals::Plldiv,
    pub divq: device::rcc::vals::Plldiv,
    pub divr: device::rcc::vals::Plldiv,
    pub cpu_div: device::rcc::vals::Hpre,
    pub ahb_div: device::rcc::vals::Hpre,
    pub apb1_div: device::rcc::vals::Ppre,
    pub apb2_div: device::rcc::vals::Ppre,
    pub apb3_div: device::rcc::vals::Ppre,
    pub apb4_div: device::rcc::vals::Ppre,
    pub flash_latency: u8,
    pub flash_write_delay: u8,
}

pub enum ClockSource {
    ExternalCrystal,
    Hsi64,
}

pub fn system_init(config: ClockConfig) {
    let cp = cortex_m::Peripherals::take().unwrap();

    system_init_custom(cp, config)
}

pub fn system_init_custom(mut cp: cortex_m::Peripherals, config: ClockConfig) {
    // Basic RAMs are working, power is stable, and the runtime has initialized
    // static variables.
    //
    // We are running at 64MHz on the HSI oscillator at voltage scale VOS3.

    // TODO What to do here?
    // stm32_metapac seems to be missing AXI configuration registers
    // {
    //     // Workaround for erratum 2.2.9 "Reading from AXI SRAM may lead to data
    //     // read corruption" - limits AXI SRAM read concurrency.
    //     p.AXI
    //         .targ7_fn_mod
    //         .modify(|_, w| w.read_iss_override().set_bit());
    // }

    // The H7 -- and perhaps the Cortex-M7 -- has the somewhat annoying
    // property that any attempt to use ITM without having TRCENA set in
    // DBGMCU results in the FIFO never being ready (that is, ITM writes
    // spin).  This is not consistent with previous generations (e.g., M3,
    // M4), but it's also not inconsistent with the docs, which explicitly
    // warn that stimulus ports are in an undefined state if TRCENA hasn't
    // been set.  So we enable tracing on ourselves as a first action, even
    // though that isn't terribly meaningful if there is no debugger to
    // consume the ITM output.  It follows from the above, but just to be
    // unequivocal: ANY use of ITM prior to this point will lock the system
    // if/when an external debugger has not set TRCENA!
    cp.DCB.enable_trace();

    // Make sure debugging works in standby.
    device::DBGMCU.cr().modify(|w| {
        w.set_d1dbgcken(true);
        w.set_dbgstby_d1(true);
        w.set_dbgstop_d1(true);
        w.set_dbgsleep_d1(true);
    });

    // Halt I2C timeout clocks when the debugger halts the system.
    device::DBGMCU.apb1lfzr1().modify(|w| {
        w.set_i2c1(true);
        w.set_i2c2(true);
        w.set_i2c3(true);
    });
    device::DBGMCU.apb4fzr1().modify(|w| {
        w.set_i2c4(true);
    });

    // Set up SYSCFG selections so drivers don't have to.
    device::RCC.apb4enr().modify(|w| w.set_syscfgen(true));
    cortex_m::asm::dmb();

    // Ethernet is on RMII, not MII.
    // p.SYSCFG.pmcr.modify(|_, w| unsafe { w.epis().bits(0b100) });
    device::SYSCFG
        .pmcr()
        .modify(|w| w.set_eth_sel_phy(stm32_metapac::syscfg::vals::EthSelPhy::RMII));

    // Turn on CPU I/D caches to improve performance at the higher clock speeds
    // we're about to enable.
    cp.SCB.enable_icache();
    cp.SCB.enable_dcache(&mut cp.CPUID);

    // The Flash controller comes out of reset configured for 3 wait states.
    // That's approximately correct for 64MHz at VOS3, which is fortunate, since
    // we've been executing instructions out of flash _the whole time._

    // Our goal is now to boost the CPU frequency to its final level. This means
    // raising the core supply voltage from VOS3 and adding wait states or
    // reduced divisors to a bunch of things, and then finally making the actual
    // change. (The target state is VOS1 on the H743/53, and VOS0 on H7B3.)

    // We're allowed to hop directly from VOS3 to the target state; the manual
    // doesn't say this explicitly but the ST drivers do it.
    //
    // We want to set the same bits on both SoCs despite the naming differences.
    // On the H7B3, the register we're calling "D3CR" here is called "SRDCR" in
    // certain editions of the manual.
    // p.PWR.d3cr.write(|w| unsafe { w.vos().bits(0b11) });
    device::PWR
        .d3cr()
        .write(|w| w.set_vos(stm32_metapac::pwr::vals::Vos::SCALE1));
    // Busy-wait for the voltage to reach the right level.
    // while !p.PWR.d3cr.read().vosrdy().bit() {
    //     // spin
    // }
    while !device::PWR.d3cr().read().vosrdy() {
        // spin
    }
    // We are now at target voltage.

    match config.source {
        ClockSource::ExternalCrystal => {
            // There's an external crystal on the board. We'll use it as our
            // clock source, to get higher accuracy than the internal
            // oscillator. To do that we must turn on the High Speed External
            // oscillator.
            device::RCC.cr().modify(|w| w.set_hseon(true));
            // Wait for it to stabilize.
            while !device::RCC.cr().read().hserdy() {
                // spin
            }

            // The clock generator divides the external crystal frequency by
            // DIVM before feeding it to the VCO, and the result must be in the
            // range 2-16MHz.
            // p.RCC
            //     .pllckselr
            //     .modify(|_, w| w.divm1().bits(config.divm).pllsrc().hse());
            device::RCC.pllckselr().modify(|w| {
                w.set_divm(1, config.divm);
                w.set_pllsrc(stm32_metapac::rcc::vals::Pllsrc::HSE);
            });

            // The VCO itself needs to be configured for the appropriate input
            // range and output range. We will also want its P-output, which is
            // the output that's tied to the system clock.
            //
            // We turn on the Q-output because it's used for a lot of peripheral
            // clocks, and the R-output for the trace unit.
            device::RCC.pllcfgr().modify(|w| {
                w.set_pllvcosel(1, config.vcosel);
                w.set_pllrge(1, config.pllrange);
                w.set_divpen(1, true);
                w.set_divqen(1, true);
                w.set_divren(1, true);
            });

            // Now, we configure the VCO for reals.
            //
            // The N value is the multiplication factor for the VCO internal
            // frequency relative to its input. The resulting internal frequency
            // must be in the range 192-836MHz. To avoid needing to configure
            // the fractional divider, we configure the VCO to 2x our target
            // frequency, 800MHz, which is in turn exactly 100x our (divided)
            // input frequency.
            //
            // The P value is the divisor from VCO frequency to system
            // frequency, so it needs to be 2 to get a 400MHz P-output.
            //
            // We set the R output to the same frequency because it's what
            // Humility currently expects, and drop the Q output for kernel
            // clock use.
            device::RCC.plldivr(1).modify(|w| {
                w.set_plln(config.divn);
                w.set_pllp(config.divp);
                w.set_pllq(config.divq);
                w.set_pllr(config.divr);
            });
        }
        ClockSource::Hsi64 => {
            device::RCC.pllckselr().write(|w| {
                w.set_pllsrc(stm32_metapac::rcc::vals::Pllsrc::HSI);
                w.set_divm(1, config.divm);
            });
            device::RCC.pllcfgr().write(|w| {
                w.set_pllvcosel(1, config.vcosel);
                w.set_pllrge(1, config.pllrange);
                w.set_divpen(1, true);
                w.set_divren(1, true);
            });
            device::RCC.plldivr(1).write(|w| {
                w.set_pllp(config.divp);
                w.set_plln(config.divn);
                w.set_pllq(config.divq);
                w.set_pllr(config.divr);
            });
        }
    }

    // Turn on PLL1 and wait for it to lock.
    device::RCC.cr().modify(|w| w.set_pllon(1, true));
    while !device::RCC.cr().read().pllrdy(1) {
        //spin
    }

    // PLL1's frequency will become the system clock, which in turn goes through
    // a series of dividers to produce clocks for each system bus.
    // Configure peripheral clock dividers to make sure we stay within
    // range when we change oscillators.
    device::RCC.d1cfgr().write(|w| {
        w.set_d1cpre(config.cpu_div);
        w.set_hpre(config.ahb_div);
        w.set_d1ppre(config.apb3_div);
    });

    // Other APB buses at HCLK/2 = CPU/4 = 100MHz
    device::RCC.d2cfgr().write(|w| {
        w.set_d2ppre1(config.apb1_div);
        w.set_d2ppre2(config.apb2_div);
    });
    device::RCC
        .d3cfgr()
        .write(|w| w.set_d3ppre(config.apb4_div));

    // Flash must be configured with wait states and programming delays to
    // conform to the target speed; see ref man Table 13
    device::FLASH.acr().write(|w| {
        w.set_latency(config.flash_latency);
        w.set_wrhighfreq(config.flash_write_delay);
    });
    loop {
        let r = device::FLASH.acr().read();
        if r.latency() == config.flash_latency && r.wrhighfreq() == config.flash_write_delay {
            break;
        }
    }

    // Not that reordering is likely here, since we polled, but: we
    // really do need the Flash to be programmed with more wait states
    // before switching the clock.
    cortex_m::asm::dmb();

    // Right! We're all set to change our clock without overclocking anything by
    // accident. Perform the switch.
    device::RCC
        .cfgr()
        .write(|w| w.set_sw(stm32_metapac::rcc::vals::Sw::PLL1_P));
    while device::RCC.cfgr().read().sws() != device::rcc::vals::Sw::PLL1_P {
        // spin
    }

    // set RNG clock to PLL1 clock
    device::RCC
        .d2ccip2r()
        .modify(|w| w.set_rngsel(stm32_metapac::rcc::vals::Rngsel::PLL1_Q));

    // Hello from target speed!
}
