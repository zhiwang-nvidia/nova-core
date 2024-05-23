// SPDX-License-Identifier: GPL-2.0

#include <drm/drm_atomic.h>

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

#endif /* CONFIG_RUST_DRM_KMS */
