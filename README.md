# psc-nanoid

[![CI status][ci badge]][ci link]
[![crates.io][crates.io badge]][crates.io link]
[![docs][docs badge]][docs link]
[![Apache 2.0 or MIT Licenses][license badge]][license link]

Generate and parse Nano IDs.

Disable default features for `no_std` plus `alloc`. Parsing, the `nanoid!` macro,
packing and `new_with(rng)` remain available; `new()` uses the host RNG and
requires the default `std` feature. `rkyv`, `serde`, `packed` and `zeroize` can
all be enabled without `std`.

```toml
psc-nanoid = { version = "^3.2.0", default-features = false, features = ["packed", "rkyv"] }
```

Archive deserialization validates the alphabet and packed size before returning an
identifier. Custom rkyv deserializers must use an error implementing
`rkyv::rancor::Source` (the usual `rkyv::rancor::Error` does). This is a
source constraint added in 3.2; the archived byte layout is unchanged.

Nano ID is a small, secure, URL-friendly, unique string ID.
Here's an example of a Nano ID:

```
qjH-6uGrFy0QgNJtUh0_c
```

This crate is a Rust implementation of the original [Nano ID](https://github.com/ai/nanoid) library written in JavaScript.
Please refer to the original library for the detailed explanation of Nano ID.

## Getting started

Add the following to your `Cargo.toml`:

```toml
[dependencies]
psc-nanoid = "^3.2.0"
```

When you want a new Nano ID, you can generate one using the [`Nanoid::new`] method.

```rust
use psc_nanoid::Nanoid;
let id: Nanoid = Nanoid::new();
```

You can parse a string into a Nano ID using [`Nanoid::try_from_str`], [`core::str::FromStr`] or [`TryFrom<String>`].

```rust
use psc_nanoid::Nanoid;
let id: Nanoid = Nanoid::try_from_str("K8N4Q7MNmeHJ-OHHoVDcz")?;
let id: Nanoid = "3hYR3muA_xvjMrrrqFWxF".parse()?;
let id: Nanoid = "iH26rJ8CpRz-gfIh7TSRu".to_string().try_into()?;
```

If the Nano ID string is constant, you can also use the [`nanoid`] macro to parse it at compile time.

```rust
use psc_nanoid::{nanoid, Nanoid};
let id = nanoid!("ClCrhcvy5kviH5ZozARfi");
const ID: Nanoid = nanoid!("9vZZWqFI_rTou3Mutq1LH");
```

The length of the Nano ID is 21 by default. You can change it by specifying the generic parameter.

```rust
use psc_nanoid::Nanoid;
let id: Nanoid<10> = "j1-SOTHHxi".parse()?;
```

You can also use a different alphabet. The list of available alphabets is in the [`alphabet`] module.

```rust
use psc_nanoid::{alphabet::Base62Alphabet, Nanoid};
let id: Nanoid<10, Base62Alphabet> = Nanoid::new();
```

## Examples

```rust
use psc_nanoid::{alphabet::Base62Alphabet, Nanoid};

// Generate a new Nano ID and print it.
let id: Nanoid = Nanoid::new();
println!("{}", id);

// Parse a string into a Nano ID and convert it back to a string.
let id: Nanoid = "abcdefg1234567UVWXYZ_".parse()?;
let s = id.to_string();

// Parse a string into a Nano ID with a different length and alphabet.
let id: Nanoid<9, Base62Alphabet> = "abc123XYZ".parse()?;
```

## Features

- `serde`: Add support for serialization and deserialization of [`Nanoid`]. Implement [`serde::Serialize`] and [`serde::Deserialize`] for [`Nanoid`].
- `zeroize`: Add support for zeroizing the memory of [`Nanoid`]. Implement [`zeroize::Zeroize`] for [`Nanoid`].

## Comparison with other implementations of Nano ID

[`nanoid`](https://docs.rs/nanoid) and [`nano-id`](https://docs.rs/nano-id) are other implementations of Nano ID in Rust.
The main difference between `nid` and the other implementations is that `nid` has [`Nanoid`] type to represent Nano IDs.
This type provides a safe way to generate and parse Nano IDs.
This is similar to [`uuid`](https://docs.rs/uuid) crate, which provides [`Uuid`](https://docs.rs/uuid/latest/uuid/struct.Uuid.html) type to represent UUIDs.

[`Nanoid::new`]: https://docs.rs/psc-nanoid/latest/psc_nanoid/struct.Nanoid.html#method.new
[`Nanoid::try_from_str`]: https://docs.rs/psc-nanoid/latest/psc_nanoid/struct.Nanoid.html#method.try_from_str
[`core::str::FromStr`]: https://doc.rust-lang.org/std/str/trait.FromStr.html
[`TryFrom<String>`]: https://doc.rust-lang.org/std/convert/trait.TryFrom.html
[`nanoid`]: https://docs.rs/psc-nanoid/latest/psc_nanoid/macro.nanoid.html
[`alphabet`]: https://docs.rs/psc-nanoid/latest/psc_nanoid/alphabet/index.html
[`Nanoid`]: https://docs.rs/psc-nanoid/latest/psc_nanoid/struct.Nanoid.html
[`serde::Serialize`]: https://docs.rs/serde/latest/serde/ser/trait.Serialize.html
[`serde::Deserialize`]: https://docs.rs/serde/latest/serde/de/trait.Deserialize.html
[`zeroize::Zeroize`]: https://docs.rs/zeroize/latest/zeroize/trait.Zeroize.html

## License

Licensed under either of

 * Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or http://www.apache.org/licenses/LICENSE-2.0)
 * MIT license ([LICENSE-MIT](LICENSE-MIT) or http://opensource.org/licenses/MIT)

at your option.

## Contribution

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in the work by you, as defined in the Apache-2.0 license, shall be dual licensed as above, without any additional terms or conditions.

[ci badge]: https://github.com/pathscale/psc-nanoid/actions/workflows/ci.yaml/badge.svg
[ci link]: https://github.com/pathscale/psc-nanoid/actions/workflows/ci.yaml

[crates.io badge]: https://img.shields.io/crates/v/psc-nanoid?logo=rust
[crates.io link]: https://crates.io/crates/psc-nanoid

[docs badge]: https://img.shields.io/badge/docs-online-teal
[docs link]: https://docs.rs/psc-nanoid

[license badge]: https://img.shields.io/badge/license-Apache--2.0_OR_MIT-blue
[license link]: #license
