// SPDX-License-Identifier: GPL-2.0 OR MIT

//! Preparation and submission from a driver's atomic commit callback.

use core::marker::PhantomData;

use crate::{
    bindings,
    drm::device::Device,
    error::{
        from_result,
        to_result, //
    },
    prelude::*,
    sync::aref::ARef,
    workqueue::{
        self,
        impl_has_work,
        new_work,
        Work,
        WorkItem, //
    },
};

use super::super::crtc::AsRawCrtcStatePrivate;

use super::{
    commit_tail_callback,
    AtomicState,
    AtomicStateMutator,
    KmsDriver, //
};

/// A validated commit before its candidate state has been installed.
///
/// Call setup, prepare the planes and driver resources, wait for input fences,
/// then swap and submit. Dropping this object before submission calls the
/// driver's abort callback and undoes successful plane preparation.
///
/// After unplug, queued work stops waiting for producer fences but still runs
/// the tail to complete cleanup. Driver callbacks must guard hardware access
/// with a device entry or registration guard.
///
/// # Invariants
///
/// The borrowed DRM commit remains live, validated and unswapped until submission.
/// The callback retains its acquire context and modeset locks for this lifetime.
/// Preparation flags record which helper operations have succeeded.
pub struct AtomicCommit<'a, T: KmsDriver> {
    state: AtomicStateMutator<T>,
    nonblock: bool,
    setup: bool,
    planes_prepared: bool,
    fences_waited: bool,
    submitted: bool,
    _callback: PhantomData<&'a mut AtomicState<T>>,
}

impl<T: KmsDriver> AtomicCommit<'_, T> {
    /// Access the candidate object states while modeset locks are held.
    pub fn state(&self) -> &AtomicStateMutator<T> {
        &self.state
    }

    /// Set up DRM commit tracking and dependencies.
    pub fn setup(&mut self) -> Result {
        if self.setup {
            return Err(EINVAL);
        }
        // SAFETY: The callback exclusively owns this live, unswapped commit.
        if unsafe { bindings::drm_atomic_commit::async_update_raw(self.state.as_raw()) } {
            return Err(EOPNOTSUPP);
        }
        // SAFETY: DRM supplied a validated commit with its modeset locks held.
        to_result(unsafe {
            bindings::drm_atomic_helper_setup_commit(self.state.as_raw(), self.nonblock)
        })?;
        self.setup = true;
        Ok(())
    }

    /// Wait for earlier CRTC commits before preparing a blocking submission.
    ///
    /// The DRM wait is bounded and reports timeouts. Nonblocking commits defer
    /// their dependency waits to the worker after setup has checked for busy CRTCs.
    pub fn wait_for_previous_commits(&mut self) -> Result {
        if !self.setup {
            return Err(EINVAL);
        }
        if self.nonblock {
            return Ok(());
        }
        for crtc in self.state.crtcs() {
            let old = self.state.get_old_crtc_state(crtc).ok_or(EINVAL)?;
            // SAFETY: The old state owns its commit reference and modeset locks
            // prevent replacement during this pre-swap wait. No display lock
            // is held here, so the earlier worker can finish its hardware work.
            to_result(unsafe { bindings::drm_crtc_commit_wait((*old.as_raw()).commit) })?;
        }
        Ok(())
    }

    /// Prepare plane resources and collect their input fences.
    pub fn prepare_planes(&mut self) -> Result {
        if !self.setup || self.planes_prepared {
            return Err(EINVAL);
        }
        // SAFETY: Setup succeeded and the mutable borrow excludes object guards
        // while the helper invokes plane callbacks. The helper unwinds failure.
        to_result(unsafe {
            bindings::drm_atomic_helper_prepare_planes(
                self.state.drm_dev().as_raw(),
                self.state.as_raw(),
            )
        })?;
        self.planes_prepared = true;
        Ok(())
    }

    /// Wait interruptibly for blocking commits; defer nonblocking waits to the tail.
    ///
    /// Producer-fence waits return `ENODEV` if the device is unplugged.
    pub fn wait_for_fences(&mut self) -> Result {
        if !self.planes_prepared {
            return Err(EINVAL);
        }
        if !self.nonblock {
            // SAFETY: The planes are prepared and the state is still unswapped.
            to_result(unsafe {
                bindings::drm_atomic_wait_for_fences_or_unplug(
                    self.state.drm_dev().as_raw(),
                    self.state.as_raw(),
                    true,
                )
            })?;
        }
        self.fences_waited = true;
        Ok(())
    }

    /// Install the candidate states and run or queue their commit tail.
    ///
    /// The work item is allocated before swapping state. Once the swap succeeds,
    /// this returns success and failures must be handled by the commit tail.
    pub fn swap_and_commit(mut self) -> Result
    where
        T: 'static,
    {
        if !self.fences_waited {
            return Err(EINVAL);
        }
        let raw = self.state.as_raw();
        // SAFETY: DRM keeps this commit alive throughout the callback. Acquire
        // another reference before handing it to the potentially queued tail.
        let state = ARef::from(unsafe { AtomicState::<T>::from_raw(raw) });
        let dev = ARef::from(self.state.drm_dev());
        let work = KBox::pin_init(
            pin_init!(CommitWork {
                state,
                dev,
                work <- new_work!("drm_atomic_commit"),
            }),
            GFP_KERNEL,
        )?;
        // SAFETY: All preparation succeeded. Consuming self excludes object
        // guards, and stall=true waits for earlier tails before replacing state.
        to_result(unsafe { bindings::drm_atomic_helper_swap_state(raw, true) })?;
        self.submitted = true;
        if self.nonblock {
            workqueue::system_dfl().enqueue(work);
        } else {
            CommitWork::run(work);
        }
        Ok(())
    }
}

impl<T: KmsDriver> Drop for AtomicCommit<'_, T> {
    fn drop(&mut self) {
        if self.submitted {
            return;
        }
        if T::HAS_ATOMIC_COMMIT_ABORT {
            T::atomic_commit_abort(&self.state);
        }
        if self.planes_prepared {
            // SAFETY: The commit was not swapped, and this undoes the successful
            // prepare_planes call once, after driver resources are released.
            unsafe {
                bindings::drm_atomic_helper_unprepare_planes(
                    self.state.drm_dev().as_raw(),
                    self.state.as_raw(),
                )
            };
        }
    }
}

#[pin_data]
struct CommitWork<T: KmsDriver> {
    state: ARef<AtomicState<T>>,
    dev: ARef<Device<T>>,
    #[pin]
    work: Work<Self>,
}

// SAFETY: This private owner is only queued after the state swap, with no live
// object guards. DRM commit dependencies serialize tail access to object state.
// KMS driver state traits require Send + Sync. Owned references keep the commit
// and device alive; hardware access still requires the registration guard.
unsafe impl<T: KmsDriver> Send for CommitWork<T> {}

impl_has_work! {
    impl{T: KmsDriver} HasWork<Self> for CommitWork<T> { self.work }
}

impl<T: KmsDriver> WorkItem for CommitWork<T> {
    type Pointer = Pin<KBox<Self>>;

    fn run(this: Self::Pointer) {
        let raw = this.state.as_raw();
        // SAFETY: This owner retains the swapped commit. An unplugged device
        // still needs its tail to retire state; callbacks guard hardware access.
        // Earlier commit dependencies must be drained even after cancellation.
        unsafe {
            bindings::drm_atomic_wait_for_fences_or_unplug(this.dev.as_raw(), raw, false);
            bindings::drm_atomic_helper_wait_for_dependencies(raw);
        }
        // SAFETY: This is the exclusive tail phase established by swap_state
        // and the dependency waits. Either tail marks hw_done and cleans planes,
        // including when unplug has canceled a producer-fence wait.
        unsafe {
            if T::HAS_ATOMIC_COMMIT_TAIL {
                commit_tail_callback::<T>(raw);
            } else {
                bindings::drm_atomic_helper_commit_tail(raw);
            }
            bindings::drm_atomic_helper_commit_cleanup_done(raw);
        }
    }
}

/// # Safety
///
/// DRM must supply a validated commit for T with its acquire context and modeset
/// locks held throughout this callback. The callback borrows DRM's reference.
pub(in crate::drm::kms) unsafe extern "C" fn atomic_commit_callback<T: KmsDriver>(
    _dev: *mut bindings::drm_device,
    state: *mut bindings::drm_atomic_commit,
    nonblock: bool,
) -> c_int {
    // SAFETY: The callback contract guarantees this pointer is live and nonnull.
    let state = unsafe { AtomicStateMutator::new(core::ptr::NonNull::new_unchecked(state)) };
    from_result(|| {
        T::atomic_commit(AtomicCommit {
            state,
            nonblock,
            setup: false,
            planes_prepared: false,
            fences_waited: false,
            submitted: false,
            _callback: PhantomData,
        })?;
        Ok(0)
    })
}
