use std::collections::BTreeMap;
use std::sync::Arc;

use super::{ChangeState, JoinPlan, Params};
use crate::{NetworkInfo, NodeIdT, PubKeyMap};

#[derive(Clone, Debug)]
pub struct Batch<C, N: Ord> {
    pub(super) epoch: u64,
    pub(super) era: u64,
    pub(super) contributions: BTreeMap<N, C>,
    pub(super) change: ChangeState<N>,
    pub(super) pub_keys: PubKeyMap<N>,
    pub(super) netinfo: Arc<NetworkInfo<N>>,
    pub(super) params: Params,
}

impl<C, N: NodeIdT> Batch<C, N> {
    pub fn epoch(&self) -> u64 {
        self.epoch
    }

    pub fn era(&self) -> u64 {
        self.era
    }

    pub fn change(&self) -> &ChangeState<N> {
        &self.change
    }

    pub fn public_keys(&self) -> &PubKeyMap<N> {
        &self.pub_keys
    }

    pub fn network_info(&self) -> &Arc<NetworkInfo<N>> {
        &self.netinfo
    }

    pub fn contributions(&self) -> impl Iterator<Item = (&N, &C)> {
        self.contributions.iter()
    }

    pub  fn iter<'a>(&'a self) -> impl Iterator<Item = <&'a C as IntoIterator>::Item> where &'a C: IntoIterator, {
        self.contributions.values().flatten()
    }

    pub fn into_tx_iter(self) -> impl Iterator<Item = <C as IntoIterator>::Item> where C: IntoIterator, {
        self.contributions.into_iter().flat_map(|(_, vec)| vec)
    }

    pub fn len<T>(&self) -> usize where C: AsRef<[T]>, {
        self.contributions.values().map(C::as_ref).map(<[T]>::len).sum()
    }    

    pub fn is_empty<T>(&self) -> bool where C: AsRef<[T]>, {
        self.contributions.values().map(C::as_ref).all(<[T]>::is_empty)
    }

    pub fn join_plan(&self) -> Option<JoinPlan<N>> {
        if self.change == ChangeState::None {
            return None;
        }
        Some (JoinPlan {
            era: self.epoch + 1,
            change: self.change.clone(),
            pub_keys: self.pub_keys.clone(),
            pub_key_set: self.netinfo.public_key_set().clone(),
            params: self.params.clone(),
        })
    }

    pub fn public_eq(&self, other: &Self) -> bool where C: PartialEq, {
        self.epoch == other.epoch && self.era == other.era 
        && self.contributions == other.contributions
        && self.change == other.change
        && self.pub_keys == other.pub_keys
        && self.netinfo.public_key_set() == other.netinfo.public_key_set()
        && self.params == other.params
    }
}