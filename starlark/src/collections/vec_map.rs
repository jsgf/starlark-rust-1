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

use crate::collections::hash::{BorrowHashed, Hashed, SmallHashResult};
use gazebo::prelude::*;
use indexmap::{Equivalent, IndexMap};
use std::{hash::BuildHasher, mem};

// TODO: benchmark, is this the right threshold
pub const THRESHOLD: usize = 12;

// We define a lot of iterators on top of other iterators
// so define a helper macro for that
macro_rules! def_iter {
    ($mapper:expr) => {
        fn next(&mut self) -> Option<Self::Item> {
            self.iter.next().map($mapper)
        }

        fn nth(&mut self, n: usize) -> Option<Self::Item> {
            self.iter.nth(n).map($mapper)
        }

        fn last(mut self) -> Option<Self::Item> {
            // Since these are all double-ended iterators we can skip to the end quickly
            self.iter.next_back().map($mapper)
        }

        fn size_hint(&self) -> (usize, Option<usize>) {
            self.iter.size_hint()
        }

        fn count(self) -> usize {
            self.iter.len()
        }

        fn collect<C>(self) -> C
        where
            C: std::iter::FromIterator<Self::Item>,
        {
            self.iter.map($mapper).collect()
        }
    };
}

#[derive(Debug, Clone, Eq, PartialEq, Default_)]
pub struct VecMap<K, V> {
    hashes: [SmallHashResult; THRESHOLD],
    values: Vec<(K, V)>,
}

pub struct VMKeys<'a, K: 'a, V: 'a> {
    iter: std::slice::Iter<'a, (K, V)>,
}

impl<'a, K: 'a, V: 'a> VMKeys<'a, K, V> {
    fn map(tup: &'a (K, V)) -> <Self as Iterator>::Item {
        &tup.0
    }
}

impl<'a, K: 'a, V: 'a> Iterator for VMKeys<'a, K, V> {
    type Item = &'a K;

    def_iter!(Self::map);
}

pub struct VMValues<'a, K: 'a, V: 'a> {
    iter: std::slice::Iter<'a, (K, V)>,
}

impl<'a, K: 'a, V: 'a> VMValues<'a, K, V> {
    fn map(tup: &'a (K, V)) -> <Self as Iterator>::Item {
        &tup.1
    }
}

impl<'a, K: 'a, V: 'a> Iterator for VMValues<'a, K, V> {
    type Item = &'a V;

    def_iter!(Self::map);
}

pub struct VMValuesMut<'a, K: 'a, V: 'a> {
    iter: std::slice::IterMut<'a, (K, V)>,
}

impl<'a, K: 'a, V: 'a> VMValuesMut<'a, K, V> {
    fn map(tup: &'a mut (K, V)) -> <Self as Iterator>::Item {
        &mut tup.1
    }
}

impl<'a, K: 'a, V: 'a> Iterator for VMValuesMut<'a, K, V> {
    type Item = &'a mut V;

    def_iter!(Self::map);
}

pub struct VMIter<'a, K: 'a, V: 'a> {
    iter: std::slice::Iter<'a, (K, V)>,
}

impl<'a, K: 'a, V: 'a> VMIter<'a, K, V> {
    fn map(tup: &(K, V)) -> (&K, &V) {
        (&tup.0, &tup.1)
    }
}

impl<'a, K: 'a, V: 'a> Iterator for VMIter<'a, K, V> {
    type Item = (&'a K, &'a V);

    def_iter!(Self::map);
}

pub struct VMIterHash<'a, K: 'a, V: 'a> {
    iter: std::iter::Zip<std::slice::Iter<'a, (K, V)>, std::slice::Iter<'a, SmallHashResult>>,
}

impl<'a, K: 'a, V: 'a> VMIterHash<'a, K, V> {
    fn map(tup: (&'a (K, V), &SmallHashResult)) -> (BorrowHashed<'a, K>, &'a V) {
        let (k, v) = tup.0;
        let h = tup.1;
        (BorrowHashed::new_unchecked(*h, k), v)
    }
}

impl<'a, K: 'a, V: 'a> Iterator for VMIterHash<'a, K, V> {
    type Item = (BorrowHashed<'a, K>, &'a V);

    def_iter!(Self::map);
}

pub struct VMIterMut<'a, K: 'a, V: 'a> {
    iter: std::slice::IterMut<'a, (K, V)>,
}

impl<'a, K: 'a, V: 'a> VMIterMut<'a, K, V> {
    fn map(tup: &mut (K, V)) -> (&K, &mut V) {
        (&tup.0, &mut tup.1)
    }
}

impl<'a, K: 'a, V: 'a> Iterator for VMIterMut<'a, K, V> {
    type Item = (&'a K, &'a mut V);

    def_iter!(Self::map);
}

pub struct VMIntoIterHash<K, V> {
    // We'd love to make a single iterator, but it's currently impossible
    // to turn a fixed array of hashes into an IntoIterator,
    // tracking it at https://github.com/rust-lang/rust/issues/25725.
    hashes: [SmallHashResult; THRESHOLD],
    iter: std::iter::Enumerate<std::vec::IntoIter<(K, V)>>,
}

impl<K, V> VMIntoIterHash<K, V> {
    // The usize is the index in self.hashes
    fn get(&self, tup: (usize, (K, V))) -> (Hashed<K>, V) {
        Self::get_hashes(&self.hashes, tup)
    }

    // The usize is the index in hashes
    fn get_hashes(hashes: &[SmallHashResult; THRESHOLD], tup: (usize, (K, V))) -> (Hashed<K>, V) {
        // The brackets below are important or rustfmt crashes,
        // see https://github.com/rust-lang/rustfmt/issues/4479
        (Hashed::new_unchecked(hashes[tup.0], (tup.1).0), (tup.1).1)
    }
}

impl<K, V> Iterator for VMIntoIterHash<K, V> {
    type Item = (Hashed<K>, V);

    fn next(&mut self) -> Option<Self::Item> {
        self.iter.next().map(|x| self.get(x))
    }

    fn nth(&mut self, n: usize) -> Option<Self::Item> {
        self.iter.nth(n).map(|x| self.get(x))
    }

    fn last(mut self) -> Option<Self::Item> {
        // Since these are all double-ended iterators we can skip to the end quickly
        self.iter.next_back().map(|x| self.get(x))
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.iter.size_hint()
    }

    fn count(self) -> usize {
        self.iter.len()
    }

    fn collect<C>(self) -> C
    where
        C: std::iter::FromIterator<Self::Item>,
    {
        let hashes = self.hashes;
        self.iter.map(|x| Self::get_hashes(&hashes, x)).collect()
    }
}

pub struct VMIntoIter<K, V> {
    iter: std::vec::IntoIter<(K, V)>,
}

impl<K, V> VMIntoIter<K, V> {
    fn map(tup: (K, V)) -> (K, V) {
        tup
    }
}

impl<'a, K: 'a, V: 'a> Iterator for VMIntoIter<K, V> {
    type Item = (K, V);

    def_iter!(Self::map);
}

impl<K, V> VecMap<K, V> {
    pub fn with_capacity(n: usize) -> Self {
        Self {
            hashes: [SmallHashResult::default(); THRESHOLD],
            values: Vec::with_capacity(n),
        }
    }

    pub fn reserve(&mut self, additional: usize) {
        self.values.reserve(additional)
    }

    pub fn get_hashed<Q>(&self, key: BorrowHashed<Q>) -> Option<&V>
    where
        Q: ?Sized + Equivalent<K>,
    {
        self.get_full(key).map(|(_, _, v)| v)
    }

    pub fn get_full<Q>(&self, key: BorrowHashed<Q>) -> Option<(usize, &K, &V)>
    where
        Q: ?Sized + Equivalent<K>,
    {
        for i in 0..self.values.len() {
            if self.hashes[i] == key.hash() {
                let v = &self.values[i];
                if key.key().equivalent(&v.0) {
                    return Some((i, &v.0, &v.1));
                }
            }
        }
        None
    }

    pub fn get_index_of_hashed<Q>(&self, key: BorrowHashed<Q>) -> Option<usize>
    where
        Q: ?Sized + Equivalent<K>,
    {
        self.get_full(key).map(|(i, _, _)| i)
    }

    pub fn get_index(&self, index: usize) -> Option<(&K, &V)> {
        self.values.get(index).map(|x| (&x.0, &x.1))
    }

    pub fn get_mut_hashed<Q>(&mut self, key: BorrowHashed<Q>) -> Option<&mut V>
    where
        Q: ?Sized + Equivalent<K>,
    {
        for i in 0..self.values.len() {
            if self.hashes[i] == key.hash() && key.key().equivalent(&self.values[i].0) {
                return Some(&mut self.values[i].1);
            }
        }
        None
    }

    pub fn contains_key_hashed<Q>(&self, key: BorrowHashed<Q>) -> bool
    where
        Q: Equivalent<K> + ?Sized,
    {
        for i in 0..self.values.len() {
            if self.hashes[i] == key.hash() && key.key().equivalent(&self.values[i].0) {
                return true;
            }
        }
        return false;
    }

    pub fn insert_hashed(&mut self, key: Hashed<K>, mut value: V) -> Option<V>
    where
        K: Eq,
    {
        if let Some(v) = self.get_mut_hashed(key.borrow()) {
            mem::swap(v, &mut value);
            Some(value)
        } else {
            let i = self.values.len();
            self.hashes[i] = key.hash();
            self.values.push((key.into_key(), value));
            None
        }
    }

    pub fn remove_hashed<Q>(&mut self, key: BorrowHashed<Q>) -> Option<V>
    where
        Q: ?Sized + Equivalent<K>,
    {
        let len = self.values.len();
        if len == 0 {
            return None;
        }

        for i in 0..len {
            if self.hashes[i] == key.hash() && key.key().equivalent(&self.values[i].0) {
                for j in i..len - 1 {
                    self.hashes[j] = self.hashes[j + 1];
                }
                return Some(self.values.remove(i).1);
            }
        }
        None
    }

    pub fn drain_to<S>(&mut self, map: &mut IndexMap<Hashed<K>, V, S>)
    where
        K: Eq,
        S: BuildHasher + Default,
    {
        let hashes = &self.hashes;
        let values = &mut self.values;

        map.extend(
            values
                .drain(..)
                .enumerate()
                .map(|(i, p)| (Hashed::new_unchecked(hashes[i], p.0), p.1)),
        );
    }

    pub fn len(&self) -> usize {
        self.values.len()
    }

    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    pub fn values(&self) -> VMValues<K, V> {
        VMValues {
            iter: self.values.iter(),
        }
    }

    pub fn values_mut(&mut self) -> VMValuesMut<K, V> {
        VMValuesMut {
            iter: self.values.iter_mut(),
        }
    }

    pub fn keys(&self) -> VMKeys<K, V> {
        VMKeys {
            iter: self.values.iter(),
        }
    }

    pub fn into_iter(self) -> VMIntoIter<K, V> {
        VMIntoIter {
            iter: self.values.into_iter(),
        }
    }

    pub fn iter(&self) -> VMIter<K, V> {
        VMIter {
            iter: self.values.iter(),
        }
    }

    pub fn iter_hashed(&self) -> VMIterHash<K, V> {
        VMIterHash {
            // Values go first since they terminate first and we can short-circuit
            iter: self.values.iter().zip(self.hashes.iter()),
        }
    }

    pub fn into_iter_hashed(self) -> VMIntoIterHash<K, V> {
        // See the comments on VMIntoIterHash for why this one looks different
        VMIntoIterHash {
            hashes: self.hashes,
            iter: self.values.into_iter().enumerate(),
        }
    }

    pub fn iter_mut(&mut self) -> VMIterMut<K, V> {
        VMIterMut {
            iter: self.values.iter_mut(),
        }
    }
}
