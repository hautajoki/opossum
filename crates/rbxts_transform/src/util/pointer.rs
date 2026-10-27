//! `util/pointer.ts`: a table under construction, built inline (`{ … }`) for as long as it can be, then moved into
//! a temporary and filled by assignments.

use rbxts_luau::{AssignmentOperator, Expr, MapField, create};

use crate::classes::prereqs::Prereqs;

/// `Pointer<T>`: a named table value. A map pointer's value is an [`Expr::Map`] or an identifier, an array pointer's
/// an [`Expr::Array`] or an identifier; class pointers start as an identifier.
#[derive(Clone, Debug)]
pub struct Pointer {
    /// The name of the temporary the value moves into.
    pub name: String,
    /// The table: inline while it is still being built in place, the temporary's identifier once it has moved.
    pub value: Expr,
}

/// `MapPointer`
pub type MapPointer = Pointer;

/// `ArrayPointer`
pub type ArrayPointer = Pointer;

/// `createMapPointer`: an empty inline map.
pub fn create_map_pointer(name: &str) -> MapPointer {
    Pointer { name: name.to_owned(), value: create::map([]) }
}

/// `createArrayPointer`: an empty inline array.
pub fn create_array_pointer(name: &str) -> ArrayPointer {
    Pointer { name: name.to_owned(), value: create::array(Vec::new()) }
}

/// `assignToMapPointer`: a field of the inline map, or `ptr[left] = right` once the map is a temporary.
pub fn assign_to_map_pointer(prereqs: &mut Prereqs, ptr: &mut Pointer, left: Expr, right: Expr) {
    if let Expr::Map(fields) = &mut ptr.value {
        fields.push(MapField { index: left, value: right });
    } else {
        prereqs.push(create::assignment(
            create::computed_index(ptr.value.clone(), left),
            AssignmentOperator::Assign,
            right,
        ));
    }
}

/// `disableMapInline`: moves an inline map into a temporary (`local _name = { … }`).
pub fn disable_map_inline(prereqs: &mut Prereqs, ptr: &mut MapPointer) {
    if matches!(ptr.value, Expr::Map(_)) {
        let value = std::mem::replace(&mut ptr.value, Expr::None);
        ptr.value = prereqs.push_to_var(value, &ptr.name);
    }
}

/// `disableArrayInline`: moves an inline array into a temporary (`local _name = { … }`).
pub fn disable_array_inline(prereqs: &mut Prereqs, ptr: &mut ArrayPointer) {
    if matches!(ptr.value, Expr::Array(_)) {
        let value = std::mem::replace(&mut ptr.value, Expr::None);
        ptr.value = prereqs.push_to_var(value, &ptr.name);
    }
}
