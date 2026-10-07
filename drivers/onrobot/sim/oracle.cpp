// The maker's own client as a process: OnRobot's `libonrobot_tool_api` (release 1.2.0, BSD-3-Clause, installed from the maker's own packages, nothing of it in this repository) driven one call at a time against a
// Modbus TCP server, and what it made of that printed as `key=value` lines. `layout_check.py` runs this against the gripper simulated in this repository, and holds this driver to it: the registers the maker's
// library reads and writes, what it decodes from them, and what it accepts and refuses before it writes (DRY-COVERAGE-PLAN.md item 12.E.8).
//
//   oracle MODEL PORT call [call...]       MODEL: RG2 RG6 2FG7 2FG14; calls: state, limits, power, setpower:W, move:W:F (RG), grip:W:F:S (2FG, external), stop
//
// One connection per run: the library validates the product code at construction, and a model that does not match is an error that is itself printed.
#include <onrobot_tool_api/rg_gripper.hpp>
#include <onrobot_tool_api/two_finger_gripper.hpp>

#include <cstdio>
#include <exception>
#include <sstream>
#include <string>
#include <vector>

static std::vector<double> numbers(const std::string &call) {
  std::vector<double> out;
  std::stringstream stream(call);
  std::string item;
  std::getline(stream, item, ':');
  while (std::getline(stream, item, ':')) out.push_back(std::stod(item));
  return out;
}

static void out(const char *key, double value) { std::printf("%s=%.9g\n", key, value); }

int main(int argc, char **argv) {
  if (argc < 3) {
    std::fprintf(stderr, "usage: oracle MODEL PORT call...\n");
    return 2;
  }
  const std::string model = argv[1];
  const int port = std::stoi(argv[2]);
  try {
    if (model == "RG2" || model == "RG6") {
      const onrobot::RgMotionLimits limits{0.0, model == "RG2" ? 110.0 : 160.0};
      onrobot::RgGripper gripper(model == "RG2" ? onrobot::Model::RG2 : onrobot::Model::RG6, onrobot::tcp("127.0.0.1", static_cast<uint16_t>(port)), limits);
      const auto identity = gripper.identity();
      std::printf("product_code=%u\nfirmware=%s\n", identity.product_code, identity.firmware_revision.c_str());
      out("maximum_force_n", gripper.maximumForceN());
      for (int i = 3; i < argc; i++) {
        const std::string call = argv[i];
        const std::string verb = call.substr(0, call.find(':'));
        const auto n = numbers(call);
        try {
          if (verb == "state") {
            const auto s = gripper.state();
            std::printf("busy=%d\ngrip_detected=%d\nsafety_1_pushed=%d\nsafety_1_triggered=%d\nsafety_2_pushed=%d\nsafety_2_triggered=%d\nsafety_dc_error=%d\nerror_code=%d\nraw_status=%u\n", s.busy, s.grip_detected,
                        s.safety_1_pushed, s.safety_1_triggered, s.safety_2_pushed, s.safety_2_triggered, s.safety_dc_error, int(s.error_code), s.raw_status);
            out("fingertip_offset_mm", s.fingertip_offset_mm);
            out("motor_voltage_v", s.motor_voltage_v);
            out("motor_current_a", s.motor_current_a);
            out("actual_depth_mm", s.actual_depth_mm);
            out("actual_relative_depth_mm", s.actual_relative_depth_mm);
            out("temperature_c", s.temperature_c);
            out("width_mm", s.width_mm);
            out("actual_width_mm", s.actual_width_mm);
            out("mechanism_angular_position_rad", s.mechanism_angular_position_rad);
            out("mechanism_angular_velocity_rad_s", s.mechanism_angular_velocity_rad_s);
            out("task_velocity_mm_s", s.task_velocity_mm_s);
          } else if (verb == "move") {
            gripper.move({n.at(0), n.at(1)});
            std::printf("move=ok\n");
          } else if (verb == "stop") {
            gripper.stop();
            std::printf("stop=ok\n");
          } else if (verb == "configuration") {
            out("long_term_power_limit_w", gripper.configuration().long_term_power_limit_w);
          } else {
            std::printf("error=unknown call %s\n", call.c_str());
          }
        } catch (const std::exception &error) {
          std::printf("%s_error=%s\n", verb.c_str(), error.what());
        }
      }
    } else {
      onrobot::TwoFingerGripper gripper(model == "2FG7" ? onrobot::Model::TwoFG7 : onrobot::Model::TwoFG14, onrobot::tcp("127.0.0.1", static_cast<uint16_t>(port)));
      const auto identity = gripper.identity();
      std::printf("product_code=%u\nfirmware=%s\n", identity.product_code, identity.firmware_revision.c_str());
      for (int i = 3; i < argc; i++) {
        const std::string call = argv[i];
        const std::string verb = call.substr(0, call.find(':'));
        const auto n = numbers(call);
        try {
          if (verb == "limits") {
            const auto l = gripper.limits();
            out("minimum_external_width_mm", l.minimum_external_width_mm);
            out("maximum_external_width_mm", l.maximum_external_width_mm);
            out("minimum_internal_width_mm", l.minimum_internal_width_mm);
            out("maximum_internal_width_mm", l.maximum_internal_width_mm);
            out("supply_power_w", l.supply_power_w);
            out("maximum_force_n", l.maximum_force_n);
            out("maximum_realtime_force_n", l.maximum_realtime_force_n);
          } else if (verb == "state") {
            const auto s = gripper.state();
            std::printf("busy=%d\ngrip_detected=%d\nnot_calibrated=%d\nlinear_sensor_error=%d\nraw_status=%u\n", s.busy, s.grip_detected, s.not_calibrated, s.linear_sensor_error, s.raw_status);
            out("external_width_mm", s.external_width_mm);
            out("internal_width_mm", s.internal_width_mm);
            out("linear_mechanism_position_mm", s.linear_mechanism_position_mm);
            out("raw_motor_width_mm", s.raw_motor_width_mm);
            out("force_n", s.force_n);
          } else if (verb == "grip") {
            gripper.grip(onrobot::GripType::External, n.at(0), n.at(1), n.at(2));
            std::printf("grip=ok\n");
          } else if (verb == "stop") {
            gripper.stop();
            std::printf("stop=ok\n");
          } else if (verb == "power") {
            const auto p = gripper.powerStatus();
            out("supply_power_w", p.supply_power_w);
            out("maximum_force_n", p.maximum_force_n);
            out("maximum_realtime_force_n", p.maximum_realtime_force_n);
          } else if (verb == "setpower") {
            const auto p = gripper.setSupplyPower(static_cast<uint16_t>(n.at(0)));
            out("supply_power_w", p.supply_power_w);
            out("maximum_force_n", p.maximum_force_n);
          } else {
            std::printf("error=unknown call %s\n", call.c_str());
          }
        } catch (const std::exception &error) {
          std::printf("%s_error=%s\n", verb.c_str(), error.what());
        }
      }
    }
  } catch (const std::exception &error) {
    std::printf("open_error=%s\n", error.what());
    return 1;
  }
  return 0;
}
