/*
 * Copyright 2019 The Starlark in Rust Authors.
 * Copyright (c) Facebook, Inc. and its affiliates.
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     https://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

// Possible optimisations:
// Preallocate int, none, bool etc slots in Value, so they are shared
// Encoding none, bool etc in the pointer of frozen value

use crate::values::{
    layout::{
        arena::Arena,
        pointer::Pointer,
        thawable_cell::ThawableCell,
        value::{immutable_static, FrozenValue, FrozenValueMem, Value, ValueMem},
        ValueRef,
    },
    AllocFrozenValue, ImmutableValue, MutableValue,
};
use gazebo::{cast, prelude::*};
use std::{
    cell::{Cell, RefCell},
    collections::HashSet,
    fmt,
    fmt::{Debug, Formatter},
    hash::{Hash, Hasher},
    ops::Deref,
    ptr,
    sync::Arc,
    time::Instant,
};

pub struct Heap {
    // Should really be ValueMem<'v>, where &'v self
    arena: RefCell<Arena<ValueMem<'static>>>,
}

impl Debug for Heap {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let mut x = f.debug_struct("Heap");
        x.field(
            "bytes",
            &self.arena().try_borrow().map(|x| x.allocated_bytes()),
        );
        x.finish()
    }
}

pub struct FrozenHeap {
    arena: Arena<FrozenValueMem>,          // My memory
    refs: RefCell<HashSet<FrozenHeapRef>>, // Memory I depend on
}

impl Debug for FrozenHeap {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let mut x = f.debug_struct("FrozenHeap");
        x.field("bytes", &self.arena.allocated_bytes());
        x.field("refs", &self.refs.try_borrow().map(|x| x.len()));
        x.finish()
    }
}

// Safe because we never mutate the Arena other than with &mut
unsafe impl Sync for FrozenHeapRef {}
unsafe impl Send for FrozenHeapRef {}

/// The Eq/Hash are by pointer rather than value
#[derive(Clone, Dupe, Debug)]
pub struct FrozenHeapRef(Arc<FrozenHeap>);

impl Hash for FrozenHeapRef {
    fn hash<H: Hasher>(&self, state: &mut H) {
        let x: &FrozenHeap = Deref::deref(&self.0);
        ptr::hash(x, state);
    }
}

impl PartialEq<FrozenHeapRef> for FrozenHeapRef {
    fn eq(&self, other: &FrozenHeapRef) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl Eq for FrozenHeapRef {}

impl FrozenHeap {
    pub fn new() -> Self {
        Self {
            arena: Arena::new(),
            refs: RefCell::new(HashSet::new()),
        }
    }

    pub fn into_ref(self) -> FrozenHeapRef {
        FrozenHeapRef(Arc::new(self))
    }

    pub fn add_reference(&self, heap: &FrozenHeapRef) {
        self.refs.borrow_mut().get_or_insert_owned(heap);
    }

    fn alloc_raw(&self, x: FrozenValueMem) -> FrozenValue {
        let v: &mut FrozenValueMem = self.arena.alloc(x);
        FrozenValue(Pointer::new_ptr1(unsafe { cast::ptr_lifetime(v) }))
    }

    pub(crate) fn alloc_str(&self, x: Box<str>) -> FrozenValue {
        self.alloc_raw(FrozenValueMem::Str(x))
    }

    pub fn alloc_immutable<'v>(&self, val: impl ImmutableValue<'v> + 'v) -> FrozenValue {
        self.alloc_raw(FrozenValueMem::Immutable(immutable_static(box val)))
    }
}

// A freezer is a pair of the FrozenHeap and a "magic" value,
// which we happen to use for the slots (see `FrozenSlotsRef`)
// but could be used for anything.
pub struct Freezer(FrozenHeap, FrozenValue);

impl Freezer {
    pub(crate) fn new(x: FrozenHeap) -> Self {
        let fv = x.alloc_raw(FrozenValueMem::Blackhole);
        Self(x, fv)
    }

    pub(crate) fn get_magic(&self) -> FrozenValue {
        self.1
    }

    pub(crate) fn set_magic(&self, val: impl ImmutableValue<'static>) {
        let p = self.1.0.unpack_ptr1().unwrap();
        let p = p as *const FrozenValueMem as *mut FrozenValueMem;
        unsafe { ptr::write(p, FrozenValueMem::Immutable(box val)) }
    }

    pub(crate) fn into_ref(self) -> FrozenHeapRef {
        self.0.into_ref()
    }

    // Not sure if this is a good idea. Let's you create brand-new things while
    // freezing. Only used in two places.
    pub fn alloc<'v, T: AllocFrozenValue<'v>>(&'v self, val: T) -> FrozenValue {
        val.alloc_frozen_value(&self.0)
    }

    pub fn freeze(&self, value: Value) -> FrozenValue {
        // Case 1: We have our value encoded in our pointer
        if let Some(x) = value.unpack_frozen() {
            return x;
        }

        // Case 2: We have already been replaced with a Forward node
        let value = value.0.unpack_ptr2().unwrap();
        match value {
            ValueMem::Forward(x) => return *x,
            ValueMem::ThawOnWrite(x) => return self.freeze(x.get_value()),
            _ => {}
        }

        // Case 3: We need to be moved to the new heap
        // Invariant: After this method completes ValueMem must be of type Forward
        let value_mut = value as *const ValueMem as *mut ValueMem;
        let fvmem: &mut FrozenValueMem = self.0.arena.alloc(FrozenValueMem::Blackhole);
        let fv = FrozenValue(Pointer::new_ptr1(unsafe { cast::ptr_lifetime(fvmem) }));
        // Important we allocate the location for the frozen value _before_ we copy it
        // so that cycles still work
        let v = unsafe { ptr::replace(value_mut, ValueMem::Forward(fv)) };

        match v {
            ValueMem::Str(i) => *fvmem = FrozenValueMem::Str(i),
            ValueMem::Immutable(x) => *fvmem = FrozenValueMem::Immutable(x),
            ValueMem::Pseudo(x) => {
                *fvmem = FrozenValueMem::Immutable(immutable_static(x.freeze(self)))
            }
            ValueMem::Mutable(x) => {
                *fvmem = FrozenValueMem::Immutable(immutable_static(x.into_inner().freeze(self)))
            }
            _ => v.unexpected("FrozenHeap::freeze case 3"),
        }
        fv
    }
}

impl Heap {
    pub fn new() -> Self {
        Self {
            arena: RefCell::new(Arena::new()),
        }
    }

    fn arena<'v>(&'v self) -> &'v RefCell<Arena<ValueMem<'v>>> {
        // Not totally safe because variance on &self might mean we create ValueMem
        // at a smaller lifetime than it has to be. But approximately correct.
        unsafe {
            transmute!(
                &'v RefCell<Arena<ValueMem<'static>>>,
                &'v RefCell<Arena<ValueMem<'v>>>,
                &self.arena
            )
        }
    }

    pub(crate) fn allocated_bytes(&self) -> usize {
        self.arena().borrow().allocated_bytes()
    }

    pub(crate) fn alloc_raw<'v>(&'v self, v: ValueMem<'v>) -> Value<'v> {
        let arena_ref = self.arena().borrow_mut();
        let arena = &*arena_ref;

        // We have an arena inside a RefCell which stores ValueMem<'v>
        // However, we promise not to clear the RefCell other than for GC
        // so we can make the `arena` available longer
        let arena = unsafe { transmute!(&Arena<ValueMem<'v>>, &'v Arena<ValueMem<'v>>, arena) };

        Value(Pointer::new_ptr2(arena.alloc(v)))
    }

    pub(crate) fn alloc_str(&self, x: Box<str>) -> Value {
        self.alloc_raw(ValueMem::Str(x))
    }

    pub fn alloc_immutable<'v>(&'v self, x: impl ImmutableValue<'v> + 'v) -> Value<'v> {
        self.alloc_raw(ValueMem::Immutable(box x))
    }

    pub fn alloc_thaw_on_write<'v>(&'v self, x: FrozenValue) -> Value<'v> {
        if x.get_ref().naturally_mutable() {
            self.alloc_raw(ValueMem::ThawOnWrite(ThawableCell::new(x)))
        } else {
            Value::new_frozen(x)
        }
    }

    pub fn alloc_mutable<'v>(&'v self, x: impl MutableValue<'v>) -> Value<'v> {
        self.alloc_mutable_box(box x)
    }

    pub(crate) fn alloc_mutable_box<'v>(&'v self, x: Box<dyn MutableValue<'v> + 'v>) -> Value<'v> {
        if x.naturally_mutable() {
            self.alloc_raw(ValueMem::Mutable(RefCell::new(x)))
        } else {
            self.alloc_raw(ValueMem::Pseudo(x))
        }
    }

    pub(crate) fn record_call_enter<'v>(&'v self, function: Value<'v>) {
        // Deliberately don't return anything - no one should ever get a Value to this
        // entry
        self.alloc_raw(ValueMem::CallEnter(function, Instant::now()));
    }

    pub(crate) fn record_call_exit(&self) {
        // Deliberately don't return anything - no one should ever get a Value to this
        // entry
        self.alloc_raw(ValueMem::CallExit(Instant::now()));
    }

    pub(crate) fn for_each<'v>(&'v self, f: impl FnMut(&'v ValueMem<'v>)) {
        let mut arena_ref = self.arena().borrow_mut();
        let arena = &mut *arena_ref;

        // We have an arena inside a RefCell which stores ValueMem<'static>
        // However, we promise not to clear the RefCell other than for GC
        // and we promise the ValueMem's are only valid for 'v
        // so we cast the `arena` to how we actually know it works
        let arena =
            unsafe { transmute!(&mut Arena<ValueMem<'v>>, &'v mut Arena<ValueMem<'v>>, arena) };

        arena.for_each(f);
    }

    /// Garbage collect any values that are unused. This function is _unsafe_ in
    /// the sense that any `Value<'v>` not returned by `Walker` _will become
    /// invalid_. Furthermore, any references to values, e.g `&'v str` will
    /// also become invalid.
    pub(crate) unsafe fn garbage_collect<'v>(&'v self, f: impl FnOnce(&Walker<'v>)) {
        self.garbage_collect_internal(f)
    }

    fn garbage_collect_internal<'v>(&'v self, f: impl FnOnce(&Walker<'v>)) {
        // Must rewrite all Value's so they point at the new heap
        let mut arena = self.arena().borrow_mut();

        let walker = Walker::<'v> {
            arena: Arena::new(),
        };
        f(&walker);
        *arena = walker.arena;
    }
}

pub struct Walker<'v> {
    arena: Arena<ValueMem<'v>>,
}

impl<'v> Walker<'v> {
    // Walk over a dictionary key that is immutable
    #[allow(clippy::trivially_copy_pass_by_ref)] // we unsafely make it a mut pointer, so the pointer matters
    pub fn walk_dictionary_key(&self, value: &Value<'v>) {
        let new_value = self.adjust(*value);
        // We are going to replace it, but promise morally it's the same Value
        // so things like Hash/Ord/Eq will work the same
        let v = value as *const Value as *mut Value;
        unsafe {
            *v = new_value
        };
    }

    // These references might be shared by multiple people, so important we only GC
    // them once per walk, or we move them twice
    pub(crate) fn walk_ref(&self, value: &ValueRef<'v>) {
        self.walk_cell(&value.0)
    }

    fn walk_cell(&self, value: &Cell<Value<'v>>) {
        value.set(self.adjust(value.get()))
    }

    pub fn walk(&self, value: &mut Value<'v>) {
        *value = self.adjust(*value)
    }

    fn adjust(&self, value: Value<'v>) -> Value<'v> {
        let old_val = value.0.unpack_ptr2();
        // Case 1, doesn't point at the old arena
        if old_val.is_none() {
            return value;
        }

        // Case 2: We have already been replaced with a Copied node
        let old_val = old_val.unwrap();
        if let ValueMem::Copied(v) = old_val {
            return *v;
        }

        // Case 3: We need to be moved to the new heap
        // Invariant: After this method completes ValueMem must be of type Copied
        let old_mem = old_val as *const ValueMem<'v> as *mut ValueMem<'v>;
        // We know the arena we are allocating into will live for 'v
        let new_mem = unsafe {
            transmute!(
                &mut ValueMem<'v>,
                &'v mut ValueMem<'v>,
                self.arena.alloc(ValueMem::Blackhole)
            )
        };
        let new_val: Value<'v> = Value(Pointer::new_ptr2(new_mem));
        let mut old_mem = unsafe { ptr::replace(old_mem, ValueMem::Copied(new_val)) };

        match &mut old_mem {
            ValueMem::Ref(x) => self.walk_cell(x),
            ValueMem::Mutable(x) => x.borrow_mut().walk(self),
            ValueMem::ThawOnWrite(x) => x.walk(self),
            ValueMem::Pseudo(x) => x.walk(self),
            _ => {} // Doesn't contain Value pointers
        }
        unsafe {
            ptr::replace(new_mem as *const ValueMem<'v> as *mut ValueMem<'v>, old_mem)
        };
        new_val
    }
}

#[test]
fn test_send_sync()
where
    FrozenHeapRef: Send + Sync,
{
}
