# Bindings

The interface every driver implements: the **RHAL**, the Robot Hardware Abstraction Layer. It is a
small C interface, so a driver can be written in almost any language.

| Folder | For |
|---|---|
| [`c/skynet_rhal.h`](c/skynet_rhal.h) | **The definition.** Everything else is generated from, or checked against, this header. |
| [`cpp/skynet_rhal.hpp`](cpp/skynet_rhal.hpp) | C++ drivers |
| [`python/skynet_rhal.py`](python/skynet_rhal.py) | Python drivers, which run under `python-host` |

Rust drivers use the `skynet-rhal` crate in [`crates/`](../crates/). To write one, start with
`skynet driver new <name>` and step 4 of the [developer guide](../docs/developer-guide.md).
