//! Evaluation facts: what roblox-ts's transformer records on Luau nodes to decide where evaluation order needs a
//! temporary (`TSTransformer/util/evaluation/facts.ts`).
//!
//! roblox-ts keeps these as symbol-keyed properties on the node objects, which survive `luau.create`'s shallow
//! clones. Here they are a [`Facts`] field on the node kinds that can carry them; `.clone()` copies them the same
//! way. The renderer ignores them. Only the data lives here: the effect algebra (joining, commuting, summarising a
//! tree) is the transformer's.
//!
//! Nodes without a `facts` field (literals, tables, `...`) never carry a fact in roblox-ts's output that changes a
//! decision: literals are primitive and effect-free by kind, and tables are never marked.

use std::collections::HashSet;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

/// What a binding read or write refers to (`Binding` in facts.ts).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Binding {
    /// A TypeScript symbol's binding (roblox-ts's `BindingKey` object, one per symbol). `id` identifies the symbol;
    /// the two flags are fixed per symbol.
    Key(BindingKey),
    /// An identifier the transformer did not resolve to a symbol: matched by name.
    Name(String),
    /// A temporary identifier: compiler-owned, so never visible to user code.
    Temp(u32),
    /// Every binding user code can read (roblox-ts's `USER_READS` wildcard).
    UserReads,
    /// Every binding user code can reassign (`USER_WRITES`).
    UserWrites,
}

/// `BindingKey`: whether a closure can observe or reassign the binding.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct BindingKey {
    /// The symbol's identity (any value unique per symbol, e.g. its address).
    pub id: u64,
    /// A closure can observe the binding.
    pub captured: bool,
    /// A closure can reassign the binding.
    pub written_by_closure: bool,
}

/// `EvaluationEffects`: the effects of executing emitted code (not of building its tree).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Effects {
    /// Bindings whose current values may be observed.
    pub reads: HashSet<Binding>,
    /// Bindings that may be reassigned (not mutations of the objects they hold).
    pub writes: HashSet<Binding>,
    /// May read mutable shared state, such as table entries or Instance properties.
    pub reads_heap: bool,
    /// May mutate shared state without reassigning a binding.
    pub writes_heap: bool,
    /// May error or fail to terminate, preventing later evaluations.
    pub throws: bool,
    /// May create a fresh identity (a table or closure) that cannot be duplicated.
    pub allocates: bool,
}

impl Effects {
    /// `NO_EFFECTS`
    pub fn none() -> Effects {
        Effects::default()
    }

    /// `UNKNOWN_EFFECTS`
    pub fn unknown() -> Effects {
        Effects {
            reads: HashSet::from([Binding::UserReads]),
            writes: HashSet::from([Binding::UserWrites]),
            reads_heap: true,
            writes_heap: true,
            throws: true,
            allocates: true,
        }
    }

    /// `MAY_THROW`
    pub fn may_throw() -> Effects {
        Effects { throws: true, ..Effects::default() }
    }

    /// `READ_HEAP`
    pub fn read_heap() -> Effects {
        Effects { throws: true, reads_heap: true, ..Effects::default() }
    }

    /// `WRITE_HEAP`
    pub fn write_heap() -> Effects {
        Effects { throws: true, reads_heap: true, writes_heap: true, ..Effects::default() }
    }

    /// `ALLOCATION`
    pub fn allocation() -> Effects {
        Effects { allocates: true, ..Effects::default() }
    }

    /// No reads, writes or flags.
    pub fn is_none(&self) -> bool {
        self.reads.is_empty()
            && self.writes.is_empty()
            && !self.reads_heap
            && !self.writes_heap
            && !self.throws
            && !self.allocates
    }
}

const CONSTANT: u8 = 1;
const PRIMITIVE: u8 = 2;
const STABLE_LOOKUP: u8 = 4;
const PURE_CONSTRUCTOR: u8 = 8;
const ROBLOX_CONSTANT: u8 = 16;
const CONSTRUCTOR_MAY_THROW: u8 = 32;

/// The facts on one node. Empty by default; cheap to clone.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Facts {
    flags: u8,
    extra: Option<Box<FactsExtra>>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct FactsExtra {
    binding: Option<BindingKey>,
    builtin: Option<Arc<Effects>>,
    call_effects: Option<Arc<Effects>>,
}

// Hashing only the flags is consistent with `Eq` (equal facts have equal flags); identifiers derive `Hash`.
impl Hash for Facts {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.flags.hash(state);
    }
}

impl Facts {
    fn extra(&mut self) -> &mut FactsExtra {
        self.extra.get_or_insert_with(Default::default)
    }

    fn set_flag(&mut self, flag: u8, value: bool) {
        if value {
            self.flags |= flag;
        } else {
            self.flags &= !flag;
        }
    }

    /// `isConstantReference`: the reference always denotes the same value, so reading it has no effect.
    pub fn is_constant(&self) -> bool {
        self.flags & CONSTANT != 0
    }

    /// `markConstantReference` (and the explicit `false` of `markBinding` for non-`const` variables).
    pub fn set_constant(&mut self, value: bool) {
        self.set_flag(CONSTANT, value);
    }

    /// The `PRIMITIVE` mark of `markPrimitiveValue`: the value is a boolean, number, string or `nil`.
    pub fn is_primitive(&self) -> bool {
        self.flags & PRIMITIVE != 0
    }

    /// `markPrimitiveValue`
    pub fn set_primitive(&mut self) {
        self.flags |= PRIMITIVE;
    }

    /// `isStableLookup`: the member lookup itself is inert.
    pub fn is_stable_lookup(&self) -> bool {
        self.flags & STABLE_LOOKUP != 0
    }

    /// `markStableLookup`
    pub fn set_stable_lookup(&mut self) {
        self.flags |= STABLE_LOOKUP;
    }

    /// rbxts-rs: the call is a Roblox datatype constructor (`UDim2.new(…)`, `Color3.fromRGB(…)`): it allocates and
    /// reads only its arguments. Only the JSX ordering reads it (`rbxts_transform`'s `get_jsx_effects`).
    pub fn is_pure_constructor(&self) -> bool {
        self.flags & PURE_CONSTRUCTOR != 0
    }

    /// rbxts-rs: the pure constructor can also throw (`new NumberRange(5, 1)`).
    pub fn constructor_may_throw(&self) -> bool {
        self.flags & CONSTRUCTOR_MAY_THROW != 0
    }

    /// rbxts-rs: marks a Roblox datatype constructor call, which may throw.
    pub fn set_pure_constructor(&mut self, may_throw: bool) {
        self.flags |= PURE_CONSTRUCTOR;
        if may_throw {
            self.flags |= CONSTRUCTOR_MAY_THROW;
        }
    }

    /// rbxts-rs: the lookup is a Roblox constant (`Enum.AutomaticSize.Y`, `Vector2.zero`): reading it has no effect.
    /// Only the JSX ordering reads it, as for [`Facts::is_pure_constructor`].
    pub fn is_roblox_constant(&self) -> bool {
        self.flags & ROBLOX_CONSTANT != 0
    }

    /// rbxts-rs: marks a Roblox constant lookup.
    pub fn set_roblox_constant(&mut self) {
        self.flags |= ROBLOX_CONSTANT;
    }

    /// The `BINDING` mark of `markBinding`.
    pub fn binding(&self) -> Option<BindingKey> {
        self.extra.as_ref()?.binding
    }

    /// Sets the `BINDING` mark.
    pub fn set_binding(&mut self, key: BindingKey) {
        self.extra().binding = Some(key);
    }

    /// `getBuiltinCallEffects`: the effects of calling this compiler-owned builtin.
    pub fn builtin(&self) -> Option<&Arc<Effects>> {
        self.extra.as_ref()?.builtin.as_ref()
    }

    /// `markBuiltinReference`: constant, with known call effects.
    pub fn set_builtin(&mut self, effects: Arc<Effects>) {
        self.flags |= CONSTANT;
        self.extra().builtin = Some(effects);
    }

    /// `getCallEffects`: the effects of calling the value.
    pub fn call_effects(&self) -> Option<&Arc<Effects>> {
        self.extra.as_ref()?.call_effects.as_ref()
    }

    /// `setCallEffects`
    pub fn set_call_effects(&mut self, effects: Arc<Effects>) {
        self.extra().call_effects = Some(effects);
    }

    /// `copyValueFacts`: the primitive mark and call effects of `from`, added to `self`.
    pub fn copy_value_facts(&mut self, from: &Facts) {
        if from.is_primitive() {
            self.set_primitive();
        }
        if let Some(effects) = from.call_effects() {
            self.set_call_effects(Arc::clone(effects));
        }
    }
}
