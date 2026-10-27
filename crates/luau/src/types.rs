//! Luau type syntax: the annotations native types put on parameters, returns and locals, and `type` aliases.
//!
//! Types are plain data like the rest of the tree. Nothing in a type is walked by the name solver, so a type may
//! only name temporary identifiers that are declared as locals of the file (a module local in
//! [`NamedType::prefix`]), never ones that only appear inside a type.

use crate::ast::{AnyIdentifier, Expr};

/// A Luau type.
#[derive(Clone, Debug, PartialEq)]
pub enum Type {
    /// `name`, `prefix.name`, with `<args>`.
    Named(Box<NamedType>),
    /// A string singleton, `"text"`; the value is unescaped.
    String(String),
    /// `true` or `false`.
    Bool(bool),
    /// `{ name: T, [K]: V }`, or `{T}` for an array.
    Table(Box<TableType>),
    /// `<G>(a: A, ...: V) -> R`
    Function(Box<FunctionType>),
    /// `A | B`
    Union(Vec<Type>),
    /// `A & B`
    Intersection(Vec<Type>),
    /// `T?`
    Optional(Box<Type>),
    /// `typeof(expression)`; the expression must be an identifier declared in the file, or a global.
    Typeof(Box<Expr>),
}

/// `prefix.name<args>`
#[derive(Clone, Debug, PartialEq)]
pub struct NamedType {
    /// A local bound to a `require` of the module that exports the type.
    pub prefix: Option<AnyIdentifier>,
    /// The type's name; dotted for a global (`Enum.Material`).
    pub name: String,
    /// The type arguments.
    pub args: Vec<Type>,
}

/// `{ fields, [key]: value }`; an indexer alone with a `number` key and no `read` renders as `{value}`.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct TableType {
    /// The named fields, in order.
    pub fields: Vec<TableTypeField>,
    /// The one indexer a table type may have.
    pub indexer: Option<Indexer>,
}

/// `[read ]name: T`
#[derive(Clone, Debug, PartialEq)]
pub struct TableTypeField {
    /// Unescaped; a name that is not an identifier renders as `["name"]`.
    pub name: String,
    /// The field's type.
    pub ty: Type,
    /// `read`: the field cannot be written.
    pub read: bool,
}

/// `[read ][key]: value`
#[derive(Clone, Debug, PartialEq)]
pub struct Indexer {
    /// The key type.
    pub key: Type,
    /// The value type.
    pub value: Type,
    /// `read`: the indexer cannot be written.
    pub read: bool,
}

/// `<generics>(parameters, ...: var_args) -> returns`
#[derive(Clone, Debug, PartialEq, Default)]
pub struct FunctionType {
    /// The generic parameters.
    pub generics: Vec<String>,
    /// Named parameters render as `name: T`.
    pub parameters: Vec<(Option<String>, Type)>,
    /// The type of `...`, when the function takes it.
    pub var_args: Option<Type>,
    /// What the function returns.
    pub returns: TypePack,
}

/// A return pack: `()`, `T`, `(A, B)`, `(A, ...V)`.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct TypePack {
    /// The fixed types, in order.
    pub types: Vec<Type>,
    /// The type of a trailing `...`.
    pub variadic: Option<Box<Type>>,
}

/// A generic parameter of an alias: `T` or `T = Default`.
#[derive(Clone, Debug, PartialEq)]
pub struct GenericParameter {
    /// The parameter's name.
    pub name: String,
    /// Its default.
    pub default: Option<Type>,
}

/// `[export ]type name<generics> = ty`
#[derive(Clone, Debug, PartialEq)]
pub struct TypeAlias {
    /// `export type`
    pub export: bool,
    /// The alias's name.
    pub name: String,
    /// Its generic parameters.
    pub generics: Vec<GenericParameter>,
    /// What it stands for.
    pub ty: Type,
}

/// The types of a function declaration or expression: `<generics>(a: A, ...: V): R`.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Signature {
    /// The generic parameters.
    pub generics: Vec<String>,
    /// One per parameter of the function (after an implicit `self`); `None` leaves one unannotated.
    pub parameters: Vec<Option<Type>>,
    /// The type of `...`, when the function takes it.
    pub var_args: Option<Type>,
    /// `None` leaves the return unannotated.
    pub returns: Option<TypePack>,
}

impl Type {
    /// A global or local type name without arguments: `number`, `any`, `Vector3`.
    pub fn named(name: impl Into<String>) -> Type {
        Type::Named(Box::new(NamedType { prefix: None, name: name.into(), args: Vec::new() }))
    }

    /// `any`
    pub fn any() -> Type {
        Type::named("any")
    }

    /// `nil`
    pub fn nil() -> Type {
        Type::named("nil")
    }

    /// `{T}`
    pub fn array(element: Type) -> Type {
        Type::Table(Box::new(TableType {
            fields: Vec::new(),
            indexer: Some(Indexer { key: Type::named("number"), value: element, read: false }),
        }))
    }

    /// A bare name with no prefix and no arguments, such as `any` or `number`.
    pub fn simple_name(&self) -> Option<&str> {
        match self {
            Type::Named(n) if n.prefix.is_none() && n.args.is_empty() => Some(&n.name),
            _ => None,
        }
    }

    /// Already admits `nil`: `T?`, `nil`, `any` or `unknown`.
    pub fn is_nilable(&self) -> bool {
        match self {
            Type::Optional(_) => true,
            Type::Union(members) => members.iter().any(Type::is_nilable),
            _ => matches!(self.simple_name(), Some("nil" | "any" | "unknown")),
        }
    }

    /// `T?`, unless `T` already admits `nil`.
    pub fn optional(self) -> Type {
        if self.is_nilable() { self } else { Type::Optional(Box::new(self)) }
    }
}

impl TypePack {
    /// A pack of one type.
    pub fn one(ty: Type) -> TypePack {
        TypePack { types: vec![ty], variadic: None }
    }
}
