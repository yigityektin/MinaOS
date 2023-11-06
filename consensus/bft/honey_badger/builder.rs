use std::default::Default;
use std::iter::once;
use std::marker::PhantomData;
use std::sync::Arc;

use crate::crypto::{SecretKey, SecretKeySet};
use serde::{de::DeserializeOwned, Serialize};

use super::{DynamicHoneyBadger, EncryptionSchedule, JoinPlan, Result, Step};
use crate::honey_badger::{Params, SubsetHandlingStrategy};
use crate::{to_pub_keys, Contribution, NetworkInfo, NodeIdT, PubKeyMap};

pub struct DynamicHoneyBadgerBuilder<C, N> {
    era: u64,
    epoch: u64,
    params: Params,
    _phantom: PhantomData<(C, N)>,
}

impl<C, N: Ord> Default for DynamicHoneyBadgerBuilder<C, N> {
    fn default() -> Self {
        DynamicHoneyBadgerBuilder {
            era: 0,
            epoch: 0,
            params: Params::default(),
            _phantom: PhantomData,
        }
    }
}

impl<C, N> DynamicHoneyBadgerBuilder<C, N> where C: Contribution + Serialize + DeserializeOwned, N: NodeIdT + Serialize + DeserializeOwned, {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn era(&mut self, era: u64) -> &mut Self {
        self.era = era;
        self
    }

    pub fn epoch(&mut self, epoch: u64) -> &mut Self {
        self.epoch = epoch;
        self
    }

    pub fn max_future_epochs(&mut self, max_future_epochs: u64) -> &mut Self {
        self.params.max_future_epochs = max_future_epochs;
        self
    }

    pub fn subset_handling_strategy(&mut self, subset_handling_strategy: SubsetHandlingStrategy,) -> &mut Self {
        self.params.subset_handling_strategy = subset_handling_strategy;
        self
    }

    pub fn encryption_schedule(&mut self, encryption_schedule: EncryptionSchedule) -> &mut Self {
        self.params.encryption_schedule = encryption_schedule;
        self
    }

    pub fn params(&mut self, params: Params) -> &mut Self {
        self.params = params;
        self
    }

    pub fn build(&mut self, netinfo: NetworkInfo<N>, secret_key: SecretKey, pub_keys: PubKeyMap<N>,) -> DynamicHoneyBadger<C, N> {
        DynamicHoneyBadger::new(secret_key, pub_keys, Arc::new(netinfo), self.params.clone(), self.era, self.epoch,)
    }

    pub fn build_first_node<R: rand::Rng>(&mut self, our_id: N, rng: &mut R,) -> Result<DynamicHoneyBadger<C, N>> {
        let sk_set = SecretKeySet::random(0, rng);
        let pk_set = sk_set.public_keys();
        let sks = sk_set.secret_key_share(0);
        let sk = rng.gen::<SecretKey>();
        let pub_keys = to_pub_keys(once((&our_id, &sk)));
        let netinfo = NetworkInfo::new(our_id.clone(), sks, pk_set, once(our_id));
        Ok(self.build(netinfo, sk, pub_keys))
    }
}

#[deprecated]
pub fn build_joining<R: rand::Rng>(&mut self, our_id: N, secret_key: SecretKey, join_plan: JoinPlan<N>, rng: &mut R,) -> Result<(DynamicHoneyBadger<C, N>, Step<C, N>)> {
    DynamicHoneyBadger::new_joining(our_id, secret_key, join_plan, rng)
}