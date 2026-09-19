//! The URScript program that turns RTDE registers into joint motion — `drivers/ur_rtde`'s
//! `control.py`, whose module header is the argument for it.
//!
//! In short: a UR owns its servo loop and the only way in at rate is `servoj`, a URScript
//! statement, so a loop runs on the controller reading six float registers and an integer gate the
//! driver writes over RTDE. `cmd == 1` servos; anything else calls `sync()`, which is what makes
//! §24's *hold* expressible — a loop that simply stopped being written would go on tracking its
//! last setpoint.
//!
//! The program is written out literally rather than templated, and `tests/vectors.rs` holds it to
//! the text `control.py` generates, character for character: the lookahead and the gain decide
//! whether the arm is smooth or unstable, and a second place that could drift from the first is a
//! second tuning.

/// The gate's value for *hold*: the program calls `sync()` and commands nothing.
pub const CMD_HOLD: i32 = 0;
/// The gate's value for *servo*: the program calls `servoj` on the six float registers.
pub const CMD_SERVO: i32 = 1;

/// Seconds one `servoj` statement covers. The driver must write a setpoint at least this often.
pub const SERVO_PERIOD_S: f64 = 0.008;

/// The program, once uploaded on the secondary interface at open.
pub const SERVO_PROGRAM: &str = "def skynet_control():
  while (True):
    cmd = read_input_integer_register(0)
    if cmd == 1:
      q = [read_input_float_register(0),
           read_input_float_register(1),
           read_input_float_register(2),
           read_input_float_register(3),
           read_input_float_register(4),
           read_input_float_register(5)]
      servoj(q, 0, 0, 0.008, 0.1, 300)
    else:
      sync()
    end
  end
end
";
