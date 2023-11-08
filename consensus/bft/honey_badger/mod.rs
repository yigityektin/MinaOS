mod batch;
mod builder;
mod change;
mod dynamic_honey_badger;

use std::collections::BTreeMap;
use serde::{Deserialize, Serialize};
use self::votes::SignedVote;
use crate::crypto::{PublicKeySet, Signature};
use crate::honey_badger::{EncryptionSchedule, Message as HbMessage, Params};
use crate::sync_key_gen::{Ack, Part, SyncKeyGen};
use crate::{NodeIdT, PubKeyMap};
pub use self::batch::Batch;
pub use self::builder::DynamicHoneyBadgerBuilder;
pub use self::Change::{Change, ChangeState};
pub use self::dynamic_honey_badger::DynamicHoneyBadger;
pub use self::error::{Error, FaultKind, Result};

pub type Step<C, N> = crate::CpStep<DynamicHoneyBadger<C, N>>;

#[derive(Clone, Debug)]
pub enum Input<C, N: Ord> {
    User(C),
    Change(Change<N>),
}

#[derive(Eq, PartialEq, Debug, Serialize, Deserialize, Hash, Clone)]
pub enum KeyGenMessage {
    Part(Part),
    Ack(Ack),
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum Message<N: Ord> {
    HoneyBadger(u64, HbMessage<N>),
    KeyGen(u64, KeyGenMessage, Box<Signature>),
    SignedVote(SignedVote<N>),
}

impl<N: Ord> Message<N> {
    fn era(&self) -> u64 {
        match *self {
            Message::HoneyBadger(era, _) => era,
            Message::KeyGen(era, _, _) => era,
            Message::SignedVote(ref signed_vote) => signed_vote.era(),
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct JoinPlan<N: Ord> {
    era: u64,
    change: ChangeState<N>,
    pub_keys: PubKeyMap<N>,
    pub_key_set: PublicKeySet,
    params: Params,
}

impl<N: Ord> JoinPlan<N> {
    pub fn next_epoch(&self) -> u64 {
        self.era
    }
}

#[derive(Debug)]
struct KeyGenState<N: Ord> {
    key_gen: SyncKeyGen<N>,
    msg_count: BTreeMap<N, usize>,
}

impl<N: NodeIdT> KeyGenState<N> {
    fn new(key_gen: SyncKeyGen<N>) -> Self {
        KeyGenState {
            key_gen, msg_count: BTreeMap::new(),
        }
    }

    fn is_ready(&self) -> bool {
        let kg = &self.key_gen;
        kg.is_ready() && kg.count_complete() * 3 > 2 * kg.public_keys().len() 
    }

    fn public_keys(&self) -> &PubKeyMap<N> {
        self.key_gen.public_keys()
    }

    fn count_messages(&mut self, node_id: &N) -> usize {
        let count = self.msg_count.entry(node_id.clone()).or_insert(0);
        *count += 1;
        *count
    }
}

#[derive(Clone, Eq, PartialEq, Debug, Serialize, Deserialize, Hash)]
pub struct InternalContrib<C, N: Ord> {
    contrib: C,
    key_gen_messages: Vec<SignedKeyGenMsg<N>>,
    votes: Vec<SignedVote<N>>,
}

#[derive(Eq, PartialEq, Debug, Serialize, Deserialize, Hash, Clone)]
struct SignedKeyGenMsg<N>(u64, N, KeygenMessage, Signature);

impl<N> SignedKeyGenMsg<N> {
    fn era(&self) -> u64 {
        self.0
    }
}