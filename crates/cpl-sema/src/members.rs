//! The member table of `spec/semantics.md` 3.9 (stdlib 10.3, S37): every
//! `@method`/`@property` stub of the standard library with a receiver,
//! keyed by (receiver type head, member name). Members are not free names:
//! they are reached only through a receiver of their head type, after
//! widening a refinement to its root base (stdlib S38).
//!
//! Struct fields and capability operations are not in this table; they are
//! the `fields` of the struct's [`cpl_hir::StructDef`] and the `ops` of the
//! capability's [`cpl_hir::CapabilityDef`].

use cpl_hir::{DefId, StdDefs, Ty};
use std::collections::BTreeMap;

/// Whether a member is read without parentheses (`xs.len`) or called
/// (`xs.take(2)`); using one as the other is `property_called` or
/// `method_not_called` (D43).
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum MemberKind {
    Method,
    Property,
}

/// The receiver heads of stdlib 10.2.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) enum TypeHead {
    Int,
    Bool,
    Text,
    Bytes,
    List,
    Option,
    Result,
}

impl TypeHead {
    /// The head named by a receiver type's path name (`List` for `List<T>`).
    pub fn from_name(name: &str) -> Option<TypeHead> {
        Some(match name {
            "Int" => TypeHead::Int,
            "Bool" => TypeHead::Bool,
            "Text" => TypeHead::Text,
            "Bytes" => TypeHead::Bytes,
            "List" => TypeHead::List,
            "Option" => TypeHead::Option,
            "Result" => TypeHead::Result,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            TypeHead::Int => "Int",
            TypeHead::Bool => "Bool",
            TypeHead::Text => "Text",
            TypeHead::Bytes => "Bytes",
            TypeHead::List => "List",
            TypeHead::Option => "Option",
            TypeHead::Result => "Result",
        }
    }

    /// The head of a type that is not a refinement (apply
    /// `Refinements::strip` first, S38); `None` for types without members.
    pub fn of(ty: &Ty, std: &StdDefs) -> Option<TypeHead> {
        match ty {
            Ty::Int => Some(TypeHead::Int),
            Ty::Bool => Some(TypeHead::Bool),
            Ty::Text => Some(TypeHead::Text),
            Ty::Bytes => Some(TypeHead::Bytes),
            Ty::List(_) => Some(TypeHead::List),
            Ty::Adt(d, _) if *d == std.option => Some(TypeHead::Option),
            Ty::Adt(d, _) if *d == std.result => Some(TypeHead::Result),
            _ => None,
        }
    }
}

/// One member: its stub definition and whether it is a property.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) struct Member {
    pub def: DefId,
    pub kind: MemberKind,
}

/// The closed member table (S22).
#[derive(Clone, Debug, Default)]
pub(crate) struct MemberTable {
    map: BTreeMap<(TypeHead, String), Member>,
}

impl MemberTable {
    /// Enter a member; `false` when (head, name) is already taken.
    pub fn insert(&mut self, head: TypeHead, name: &str, member: Member) -> bool {
        use std::collections::btree_map::Entry;
        match self.map.entry((head, name.to_string())) {
            Entry::Occupied(_) => false,
            Entry::Vacant(v) => {
                v.insert(member);
                true
            }
        }
    }

    /// The member `name` of `head`.
    pub fn lookup(&self, head: TypeHead, name: &str) -> Option<Member> {
        self.map.get(&(head, name.to_string())).copied()
    }

    /// Every member of `head`, by name.
    #[allow(dead_code)] // for diagnostic notes listing a type's members
    pub fn members_of(&self, head: TypeHead) -> impl Iterator<Item = (&str, Member)> {
        self.map
            .iter()
            .filter(move |((h, _), _)| *h == head)
            .map(|((_, n), m)| (n.as_str(), *m))
    }
}
