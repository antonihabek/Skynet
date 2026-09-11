// SHA-256 (FIPS 180-4). See `src/sha256.cpp` for why this edition writes its own.

#ifndef SKYNET_OS_SHA256_HPP
#define SKYNET_OS_SHA256_HPP

#include <cstdint>
#include <string>
#include <string_view>

namespace skynet {

/// The digest of `message`, as 64 lowercase hexadecimal characters.
[[nodiscard]] std::string sha256_hex(std::string_view message);

}  // namespace skynet

#endif  // SKYNET_OS_SHA256_HPP
