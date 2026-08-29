// The C++ wrapper is vendor-facing: a ROS 2 shop implementing a driver reads it before deciding
// whether the platform is worth the afternoon. So it has to compile, and this is what says so.
//
// Syntax-only. There is nothing to run — the check is that the header is includable from C++ and
// that the C ABI it wraps is reachable through `extern "C"`.
#include "skynet_rhal.hpp"
#include "skynet_rhal.h"

static_assert(SKYNET_RHAL_ABI_VERSION == 1, "the wrapper targets ABI v1");
static_assert(sizeof(skr_driver) == 11 * sizeof(void*), "vtable shape changed");
