// SPDX-License-Identifier: GPL-2.0
//! RVKMS's DRM CRTC implementation.
use super::RvkmsDriver;
use core::marker::*;
use kernel::{
    drm::{
        device::Device,
        kms::{
            atomic::*,
            crtc::{self, DriverCrtcOps, RawCrtcState},
            vblank::*,
            KmsRef, ModeObject,
        },
    },
    impl_has_hr_timer,
    interrupt::LocalInterruptDisabled,
    new_spinlock_irq,
    prelude::*,
    sync::{Arc, SpinLockIrq},
    time::{hrtimer::*, Delta, Instant, Monotonic},
};

pub(crate) type Crtc = crtc::Crtc<RvkmsCrtc>;
pub(crate) type UnregisteredCrtc = crtc::UnregisteredCrtc<RvkmsCrtc>;
pub(crate) type CrtcState = crtc::CrtcState<RvkmsCrtcState>;

#[derive(Default)]
pub(crate) struct VblankState {
    /// A reference to the current VblankTimer
    timer: Option<Arc<VblankTimer>>,

    /// A reference to a handle for the current VblankTimer
    handle: Option<ArcHrTimerHandle<VblankTimer>>,

    /// The current frame duration in ns
    ///
    /// Stored separately here so it can be read safely without the vblank lock
    period_ns: i32,
}

#[pin_data]
pub(crate) struct RvkmsCrtc {
    /// The current vblank emulation state
    ///
    /// This is uninitalized when the CRTC is disabled to prevent circular references
    #[pin]
    vblank_state: SpinLockIrq<VblankState>,
}

#[vtable]
impl crtc::DriverCrtc for RvkmsCrtc {
    #[unique]
    const OPS: &'static DriverCrtcOps;

    type Args = ();
    type State = RvkmsCrtcState;
    type Driver = RvkmsDriver;
    type VblankImpl = Self;

    fn new(_device: &Device<Self::Driver>, _args: &Self::Args) -> impl PinInit<Self, Error> {
        try_pin_init!(Self {
            vblank_state <- new_spinlock_irq!(VblankState::default(), "vblank_handle_lock")
        })
    }

    fn atomic_check(
        crtc: &Crtc,
        old_state: &CrtcState,
        mut new_state: crtc::CrtcStateMutator<'_, CrtcState>,
        state: &AtomicStateComposer<Self::Driver>,
    ) -> Result {
        state.add_affected_planes(crtc)?;

        // Create a vblank timer when enabling a CRTC, and destroy said timer when disabling to
        // resolve the circular reference to CRTC it creates
        if old_state.active() != new_state.active() {
            new_state.vblank_timer = if new_state.active() {
                Some(VblankTimer::new(crtc)?)
            } else {
                None
            };
        }

        Ok(())
    }

    fn atomic_flush(
        crtc: &Crtc,
        _old_state: &CrtcState,
        mut new_state: crtc::CrtcStateMutator<'_, CrtcState>,
        _state: &AtomicStateMutator<Self::Driver>,
    ) {
        if let Some(event) = new_state.get_pending_vblank_event() {
            if let Ok(vbl_ref) = crtc.vblank_get() {
                event.arm(vbl_ref);
            } else {
                event.send();
            }
        }
    }

    fn atomic_enable(
        crtc: &Crtc,
        _old_state: &CrtcState,
        new_state: crtc::CrtcStateMutator<'_, CrtcState>,
        _state: &AtomicStateMutator<Self::Driver>,
    ) {
        // Store a reference to the newly created vblank timer for this CRTC
        crtc.vblank_state.lock().timer = new_state.vblank_timer.clone();

        crtc.vblank_on();
    }

    fn atomic_disable(
        crtc: &Crtc,
        _old_state: &CrtcState,
        _new_state: crtc::CrtcStateMutator<'_, CrtcState>,
        _state: &AtomicStateMutator<Self::Driver>,
    ) {
        crtc.vblank_off();

        let mut vbl_state = crtc.vblank_state.lock();
        let handles = (vbl_state.timer.take(), vbl_state.handle.take());

        // Since we just explicitly disabled vblanks, destroy the vblank state to resolve circular
        // reference to this CRTC that it holds.
        // Note that since dropping the handle will cause us to wait for the timer to finish, we
        // must drop the lock before we do so.
        drop(vbl_state);
        drop(handles);
    }
}

impl VblankSupport for RvkmsCrtc {
    type Crtc = Self;

    fn enable_vblank(
        crtc: &Crtc,
        vblank: &VblankGuard<'_, Self::Crtc>,
        irq: &LocalInterruptDisabled,
    ) -> Result {
        let period_ns = vblank.frame_duration();
        let mut vbl_state = crtc.vblank_state.lock_with(irq);

        if let Some(timer) = vbl_state.timer.clone() {
            vbl_state.period_ns = period_ns;
            vbl_state.handle = Some(timer.start(Delta::from_nanos(i64::from(period_ns))));
        }

        Ok(())
    }

    fn disable_vblank(
        crtc: &Crtc,
        _vbl_guard: &VblankGuard<'_, Self::Crtc>,
        irq: &LocalInterruptDisabled,
    ) {
        let handle = crtc.vblank_state.lock_with(irq).handle.take();

        // Now that we're outside of the vblank lock, we can safely drop the handle
        drop(handle);
    }

    fn get_vblank_timestamp(crtc: &Crtc, _handling_vblank_irq: bool) -> Option<VblankTimestamp> {
        let vbl_state = crtc.vblank_state.lock();

        // Return the expiration of our vblank timer if we have one (if not, vblanks are disabled)
        let time = vbl_state.timer.as_ref().map(|t| {
            // To prevent races, we roll the hrtimer forward before we do any interrupt
            // processing - this is how real hw works (the interrupt is only generated after all
            // the vblank registers are updated) and what the vblank core expects. Therefore we
            // need to always correct the timestamps by one frame.
            t.timer.expires() - Delta::from_nanos(i64::from(vbl_state.period_ns))
        });

        Some(VblankTimestamp {
            // …otherwise, just use the current time
            time: time.unwrap_or_else(Instant::<Monotonic>::now),
            max_error: 0,
        })
    }
}

#[derive(Clone, Default)]
pub(crate) struct RvkmsCrtcState {
    /// The current hrtimer used for emulating vblank events, if there is one.
    vblank_timer: Option<Arc<VblankTimer>>,
}

impl crtc::DriverCrtcState for RvkmsCrtcState {
    type Crtc = RvkmsCrtc;
}

/// The main hrtimer structure for emulating vblanks.
#[pin_data]
pub(crate) struct VblankTimer {
    /// The actual hrtimer used for sending out vblanks
    #[pin]
    timer: HrTimer<Self>,

    /// An owned reference to the CRTC that this [`VblankTimer`] belongs to
    crtc: KmsRef<Crtc>,
}

impl_has_hr_timer! {
    impl HasHrTimer<Self> for VblankTimer {
        mode: RelativeMode<Monotonic>,
        field: self.timer,
    }
}

impl VblankTimer {
    pub(crate) fn new(crtc: &Crtc) -> Result<Arc<Self>> {
        Arc::pin_init(
            pin_init!(Self {
                timer <- HrTimer::<Self>::new(),
                crtc: crtc.into(),
            }),
            GFP_KERNEL,
        )
    }
}

impl HrTimerCallback for VblankTimer {
    type Pointer<'a> = Arc<Self>;

    fn run(
        this: <Self::Pointer<'_> as RawHrTimerCallback>::CallbackTarget<'_>,
        mut context: HrTimerCallbackContext<'_, Self>,
    ) -> HrTimerRestart
    where
        Self: Sized,
    {
        let period_ns = this.crtc.vblank_state.lock().period_ns;

        let overrun = context.forward_now(Delta::from_nanos(i64::from(period_ns)));
        if overrun != 1 {
            dev_warn!(
                this.crtc.drm_dev().as_ref(),
                "vblank timer overrun (expected 1, got {overrun})\n"
            );
        }

        this.crtc.handle_vblank();

        HrTimerRestart::Restart
    }
}
