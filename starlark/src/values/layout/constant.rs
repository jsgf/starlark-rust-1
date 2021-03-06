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

use crate::values::layout::{
    pointer::Pointer,
    value::{FrozenValue, FrozenValueMem},
};
use once_cell::sync::OnceCell;

/// These values will be declared globally at the top-level
pub struct ConstFrozenValue(&'static str, OnceCell<FrozenValueMem>);

impl ConstFrozenValue {
    pub const fn new(name: &'static str) -> Self {
        ConstFrozenValue(name, OnceCell::new())
    }

    pub fn unpack(&'static self) -> FrozenValue {
        let v = self
            .1
            .get_or_init(|| FrozenValueMem::Str(self.0.to_owned().into_boxed_str()));
        FrozenValue(Pointer::new_ptr1(v))
    }
}
