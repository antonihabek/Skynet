// SHA-256 (FIPS 180-4).
//
// Written out because C++ has no standard hash and this edition takes no dependencies: §31.2 keeps
// the *formats* single-source across editions and a hash function is not one of them — every
// edition computes the same digest from the same bytes or it is not an edition. Python reaches for
// `hashlib`, Rust for `crates/skynet-sha256`, and this is the C++ answer.
//
// The corpus is the check that matters. `corpus/digest` pins the digest of a version end to end, so
// a transcription error here surfaces as a failing case rather than as a package whose identity is
// one thing on the robot and another in the registry.

#include "skynet_os/sha256.hpp"

#include <array>
#include <cstdio>

namespace skynet {
namespace {

constexpr std::array<std::uint32_t, 64> kK = {
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
};

std::uint32_t rotate_right(std::uint32_t value, int by) {
  return (value >> by) | (value << (32 - by));
}

}  // namespace

std::string sha256_hex(std::string_view message) {
  std::array<std::uint32_t, 8> state = {0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a,
                                        0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19};

  // Padding: the message, a 0x80 byte, zeros, then the bit length as a big-endian 64-bit integer.
  std::string padded(message);
  const std::uint64_t bit_length = static_cast<std::uint64_t>(message.size()) * 8;
  padded.push_back(static_cast<char>(0x80));
  while (padded.size() % 64 != 56) {
    padded.push_back('\0');
  }
  for (int shift = 56; shift >= 0; shift -= 8) {
    padded.push_back(static_cast<char>((bit_length >> shift) & 0xFF));
  }

  std::array<std::uint32_t, 64> schedule{};
  for (std::size_t at = 0; at < padded.size(); at += 64) {
    for (std::size_t word = 0; word < 16; ++word) {
      const std::size_t base = at + word * 4;
      schedule[word] = (static_cast<std::uint32_t>(static_cast<unsigned char>(padded[base])) << 24) |
                       (static_cast<std::uint32_t>(static_cast<unsigned char>(padded[base + 1])) << 16) |
                       (static_cast<std::uint32_t>(static_cast<unsigned char>(padded[base + 2])) << 8) |
                       static_cast<std::uint32_t>(static_cast<unsigned char>(padded[base + 3]));
    }
    for (std::size_t word = 16; word < 64; ++word) {
      const std::uint32_t s0 = rotate_right(schedule[word - 15], 7) ^
                               rotate_right(schedule[word - 15], 18) ^ (schedule[word - 15] >> 3);
      const std::uint32_t s1 = rotate_right(schedule[word - 2], 17) ^
                               rotate_right(schedule[word - 2], 19) ^ (schedule[word - 2] >> 10);
      schedule[word] = schedule[word - 16] + s0 + schedule[word - 7] + s1;
    }

    std::array<std::uint32_t, 8> working = state;
    for (std::size_t round = 0; round < 64; ++round) {
      const std::uint32_t s1 = rotate_right(working[4], 6) ^ rotate_right(working[4], 11) ^
                               rotate_right(working[4], 25);
      const std::uint32_t choose =
          (working[4] & working[5]) ^ (~working[4] & working[6]);
      const std::uint32_t temp1 = working[7] + s1 + choose + kK[round] + schedule[round];
      const std::uint32_t s0 = rotate_right(working[0], 2) ^ rotate_right(working[0], 13) ^
                               rotate_right(working[0], 22);
      const std::uint32_t majority =
          (working[0] & working[1]) ^ (working[0] & working[2]) ^ (working[1] & working[2]);
      const std::uint32_t temp2 = s0 + majority;

      working[7] = working[6];
      working[6] = working[5];
      working[5] = working[4];
      working[4] = working[3] + temp1;
      working[3] = working[2];
      working[2] = working[1];
      working[1] = working[0];
      working[0] = temp1 + temp2;
    }
    for (std::size_t index = 0; index < 8; ++index) {
      state[index] += working[index];
    }
  }

  std::string out;
  out.reserve(64);
  char buffer[9];
  for (const std::uint32_t word : state) {
    std::snprintf(buffer, sizeof(buffer), "%08x", word);
    out += buffer;
  }
  return out;
}

}  // namespace skynet
