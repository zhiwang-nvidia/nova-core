// SPDX-License-Identifier: GPL-2.0

#include <linux/dma-fence.h>
#include <linux/jiffies.h>
#include <drm/drm_atomic.h>
#include <drm/drm_drv.h>

#ifdef CONFIG_RUST_DRM_KMS

__rust_helper void rust_helper_drm_atomic_commit_get(struct drm_atomic_commit *state)
{
	drm_atomic_commit_get(state);
}

__rust_helper void rust_helper_drm_atomic_commit_put(struct drm_atomic_commit *state)
{
	drm_atomic_commit_put(state);
}

/* Generate the inline accessors for old and new mode-object states. */
#define STATE_FUNC(type, tense) \
	__rust_helper struct drm_##type##_state * \
	rust_helper_drm_atomic_get_##tense##_##type##_state( \
		const struct drm_atomic_commit *state, struct drm_##type *type) \
	{ \
		return drm_atomic_get_##tense##_##type##_state(state, type); \
	}

STATE_FUNC(plane, new)
STATE_FUNC(plane, old)
STATE_FUNC(crtc, new)
STATE_FUNC(crtc, old)
STATE_FUNC(connector, new)
STATE_FUNC(connector, old)

#undef STATE_FUNC

/* Like the native wait, retain any unconsumed fence on failure. */
__rust_helper int
rust_helper_drm_atomic_wait_for_fences_or_unplug(struct drm_device *dev,
						 struct drm_atomic_commit *state,
						 bool pre_swap)
{
	struct drm_plane *plane;
	struct drm_plane_state *new_plane_state;
	long ret;
	int i;

	for_each_new_plane_in_state(state, plane, new_plane_state, i) {
		if (!new_plane_state->fence)
			continue;

		WARN_ON(!new_plane_state->fb);
		do {
			if (drm_dev_is_unplugged(dev))
				return -ENODEV;
			ret = dma_fence_wait_timeout(new_plane_state->fence, pre_swap,
						     msecs_to_jiffies(100));
			if (ret < 0)
				return ret;
		} while (!ret);

		dma_fence_put(new_plane_state->fence);
		new_plane_state->fence = NULL;
	}

	return 0;
}

#endif /* CONFIG_RUST_DRM_KMS */
