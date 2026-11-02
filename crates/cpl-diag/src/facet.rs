//! The facet vocabulary (`spec/toolchain.md` 1.4 and 5; semantics 1.3):
//! the eight facets every declaration has, and the selector form that adds
//! `.definition` (all of them).

use serde::de::{self, Deserializer, Visitor};
use serde::{Deserialize, Serialize, Serializer};
use std::fmt;
use std::str::FromStr;

/// One facet of a declaration. The derived order is the canonical order
/// used by every facet map and by the `.definition` hash.
#[derive(Copy, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Facet {
    Identity,
    Name,
    Signature,
    Contract,
    Effects,
    Body,
    Docs,
    VerificationPolicy,
}

impl Facet {
    /// Canonical order used by every map and by the `.definition` hash.
    pub const ALL: [Facet; 8] = [
        Facet::Identity,
        Facet::Name,
        Facet::Signature,
        Facet::Contract,
        Facet::Effects,
        Facet::Body,
        Facet::Docs,
        Facet::VerificationPolicy,
    ];

    /// `"identity"`, ..., `"verification_policy"`.
    pub fn as_str(self) -> &'static str {
        match self {
            Facet::Identity => "identity",
            Facet::Name => "name",
            Facet::Signature => "signature",
            Facet::Contract => "contract",
            Facet::Effects => "effects",
            Facet::Body => "body",
            Facet::Docs => "docs",
            Facet::VerificationPolicy => "verification_policy",
        }
    }

    /// The facet with this spelling (`definition` is not a facet; see
    /// [`FacetSelector::parse`]).
    pub fn parse(s: &str) -> Option<Facet> {
        Facet::ALL.into_iter().find(|f| f.as_str() == s)
    }
}

impl fmt::Display for Facet {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Facet {
    type Err = String;
    fn from_str(s: &str) -> Result<Facet, String> {
        Facet::parse(s).ok_or_else(|| format!("unknown facet `{s}`"))
    }
}

/// What a selector's facet part names: one facet, or `.definition` (every
/// facet, D38).
///
/// Serialized as a plain string, `"body"` or `"definition"`, as in the
/// `targets` of `cpl work check` (Section 7.2).
#[derive(Copy, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
pub enum FacetSelector {
    Facet(Facet),
    Definition,
}

impl FacetSelector {
    /// The facet spelling, or `"definition"`.
    pub fn as_str(self) -> &'static str {
        match self {
            FacetSelector::Facet(f) => f.as_str(),
            FacetSelector::Definition => "definition",
        }
    }

    /// A facet spelling or `definition`.
    pub fn parse(s: &str) -> Option<FacetSelector> {
        if s == "definition" {
            Some(FacetSelector::Definition)
        } else {
            Facet::parse(s).map(FacetSelector::Facet)
        }
    }

    /// The facets selected, in canonical order (all eight for `Definition`).
    pub fn facets(self) -> Vec<Facet> {
        match self {
            FacetSelector::Facet(f) => vec![f],
            FacetSelector::Definition => Facet::ALL.to_vec(),
        }
    }

    /// True when `facet` is selected.
    pub fn includes(self, facet: Facet) -> bool {
        match self {
            FacetSelector::Facet(f) => f == facet,
            FacetSelector::Definition => true,
        }
    }
}

impl From<Facet> for FacetSelector {
    fn from(f: Facet) -> Self {
        FacetSelector::Facet(f)
    }
}

impl fmt::Display for FacetSelector {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for FacetSelector {
    type Err = String;
    fn from_str(s: &str) -> Result<FacetSelector, String> {
        FacetSelector::parse(s).ok_or_else(|| format!("unknown facet `{s}`"))
    }
}

impl Serialize for FacetSelector {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for FacetSelector {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct V;
        impl Visitor<'_> for V {
            type Value = FacetSelector;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a facet name or \"definition\"")
            }
            fn visit_str<E: de::Error>(self, s: &str) -> Result<FacetSelector, E> {
                FacetSelector::parse(s).ok_or_else(|| E::custom(format!("unknown facet `{s}`")))
            }
        }
        deserializer.deserialize_str(V)
    }
}
