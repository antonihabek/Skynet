/**
 * The one type the reference modules import from outside themselves.
 *
 * On the Hub this lives in `lib/skynet/types.ts` alongside the rest of the marketplace's object
 * model. Here it is restated so the oracle is standalone: `node reference/typescript/*.ts` must
 * run with nothing else installed and nothing else present, because a corpus generator that
 * needs a Next.js application checked out beside it is a corpus generator nobody runs.
 */

export interface SafetyEnvelope {
  /** Per-joint angular velocity ceiling. */
  joint_velocity_max_rad_s?: number
  /** Per-joint torque ceiling. */
  joint_torque_max_nm?: number
  /** Whole-body translation ceiling. */
  base_velocity_max_m_s?: number
  /** Command timeout before the controller faults, in milliseconds. */
  watchdog_ms?: number
  /** Whether a hardware emergency stop is fitted. */
  estop?: boolean
  /** Where the figures came from — a datasheet URL. A limit with no provenance is a rumour. */
  source?: string
}
