#![cfg(all(feature = "rkyv", feature = "packed"))]

use pretty_assertions::assert_eq;
use psc_nanoid::{alphabet::Base62Alphabet, packed::PackError, Nanoid, PackedNanoid};
use rkyv::rancor::Error;

#[test]
fn archived_nanoid_rejects_non_ascii_and_wrong_alphabet() {
    for bytes in [[0xff; 16], [b'!'; 16]] {
        let bytes = rkyv::to_bytes::<Error>(&bytes).unwrap();
        assert!(rkyv::from_bytes::<Nanoid<16, Base62Alphabet>, Error>(&bytes).is_err());
    }
    let id: Nanoid<16, Base62Alphabet> = "1234567890abcdef".parse().unwrap();
    let bytes = rkyv::to_bytes::<Error>(&id).unwrap();
    assert_eq!(
        rkyv::from_bytes::<Nanoid<16, Base62Alphabet>, Error>(&bytes).unwrap(),
        id
    );
}

#[test]
fn packed_length_must_match_the_type() {
    let id: Nanoid = "qjH-6uGrFy0QgNJtUh0_c".parse().unwrap();
    assert!(matches!(
        PackedNanoid::<21, 15, _>::pack(&id),
        Err(PackError::InvalidLength {
            expected: 16,
            actual: 15
        })
    ));
    assert!(matches!(
        PackedNanoid::<21, 17, _>::pack(&id),
        Err(PackError::InvalidLength {
            expected: 16,
            actual: 17
        })
    ));
}

#[test]
fn archived_packed_nanoid_rejects_bad_indices_and_lengths() {
    let bytes = rkyv::to_bytes::<Error>(&[0xffu8; 12]).unwrap();
    assert!(rkyv::from_bytes::<PackedNanoid<16, 12, Base62Alphabet>, Error>(&bytes).is_err());
    let bytes = rkyv::to_bytes::<Error>(&[0u8; 11]).unwrap();
    assert!(rkyv::from_bytes::<PackedNanoid<16, 11, Base62Alphabet>, Error>(&bytes).is_err());
}
