"""What a person does at the controller before an arm can be driven: read the error it starts with, say what it is,
clear it, and enable the motors.

    XARM_SDK=/opt/xarm-sdk python3 clear_startup_error.py HOST

`drivers/xarm` refuses to come up over a latched error, on purpose: clearing it in the driver would erase the one piece of
evidence about why the arm stopped, on every restart (`skynet_xarm/bridge.py`). So the step that clears one belongs to
somebody, and here it is done with **UFACTORY's own client**, out loud, before the driver is started. Nothing here is
hidden from the run: the code, the vendor's own title for it and its advice are printed, and a controller that still
holds an error afterwards is a failure.

Found by DRY-COVERAGE-PLAN.md F181: the xArm 5 firmware in UFACTORY's simulator image starts with error 41, *"The content
of the friction file is invalid"* (the image carries no friction file for it). On a real arm that message means restart
with the emergency-stop button; on a simulation with no motors it means the file the simulator did not ship.
"""

from __future__ import annotations

import os
import sys
import time

sdk = os.environ.get("XARM_SDK")
if not sdk:
    raise SystemExit("XARM_SDK does not name a checkout of UFACTORY's xArm-Python-SDK")
sys.path.insert(0, sdk)

from xarm.wrapper import XArmAPI  # noqa: E402

host = sys.argv[1]
arm = XArmAPI(host, is_radian=True, do_not_open=False)
time.sleep(1.0)
print(f"the controller starts with error {arm.error_code} and warning {arm.warn_code}, state {arm.state}")
if arm.error_code:
    try:
        from xarm.core.config.x_code import ControllerErrorCodeMap  # noqa: PLC0415

        entry = ControllerErrorCodeMap.get(arm.error_code, {}).get("en", {})
        print(f"  the vendor's title: {entry.get('title')}")
        print(f"  the vendor's advice: {entry.get('desc')}")
    except Exception:  # the table is the vendor's; a moved name must not hide the code above
        pass
if not arm.error_code and not arm.warn_code:
    # A controller that starts clean is not touched: the driver is measured against it as it is.
    arm.disconnect()
    raise SystemExit(0)
arm.clean_error()
arm.clean_warn()
arm.motion_enable(True)
arm.set_mode(0)
arm.set_state(0)
time.sleep(1.5)
print(f"after the operator's step: error {arm.error_code}, warning {arm.warn_code}, state {arm.state}")
arm.disconnect()
raise SystemExit(1 if arm.error_code else 0)
