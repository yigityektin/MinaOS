use std::collections::{BTreeMap, HashMap};
use crate::crypto::{SecretKey, Signature};
use bincode;
use serde::{Deserialize, Serialize};
use super::{Change, Error, FaultKind, Result};
use crate::{fault_log, util, NodeIdT, PubKeyMap};

pub type FaultLog<N> = fault_log::FaultLog<N, FaultKind>;

#[derive(Debug)]
pub struct VoteCounter<N: Ord> {
    our_id: N,
    secret_key: SecretKey,
    pub_keys: PubKeyMap<N>,
    era: u64,
    pending: BTreeMap<N, SignedVote<N>>,
    committed: BTreeMap<N, Vote<N>>,
}

impl<N> VoteCounter<N> where N: NodeIdT + Serialize, {
    pub fn new(our_id: N, secret_key: SecretKey, pub_keys: PubKeyMap<N>, era: u64) -> Self {
        VoteCounter {
            our_id, secret_key, pub_keys, era, pending: BTreeMap::new(), committed: BTreeMap::new(),
        }
    }

    pub fn sing_vote_for(&mut self, change: Change<N>) -> Result<&SignedVote<N>> {
        let voter = self.our_id.clone();
        let vote = Vote {
            change,
            era: self.era,
            num: self.pending.get(&voter).map_or(0, |sv| sv.vote.num + 1),
        };

        let ser_vote = bincode::serialize(&vote).map_err(|err| Error::SerializeVote(*err))?;
        let signed_vote = SignedVote {
            vote,
            voter: voter.clone(),
            sig: self.secret_key.sign(ser_vote),
        };
        self.pending.remove(&voter);
        Ok(self.pending.entry(voter).or_insert(signed_vote))
    }

    pub fn add_pending_vote(&mut self, sender_id: &N, signed_vote: SignedVote<N>) -> Result<FaultLog<N>> {
        if signed_vote.vote.era != self.era || self.pending.get(&signed_vote.voter).map_or(false, |sv| sv.vote.num >= signed_vote.vote.num) {
            Ok(FaultLog::new());
        }
        if !self.validate(&signed_vote)? {
            return Ok(FaultLog::init(
                sender_id.clone(),
                FaultKind::InvalidVoteSignature,
            ));
        }
        self.pending.insert(signed_vote.voter.clone(), signed_vote);
        Ok(FaultLog::new())
    }

    pub fn pending_votes(&self) -> impl Iterator<Item = &SignedVote<N>> {
        self.pending.values().filter(move |signed_vote| {
            self.committed.get(&signed_vote.voter).map_or(true, |vote| vote.num < signed_vote.vote.num)
        })
    }

    pub fn add_committed_votes<I>(&mut self, proposer_id: &N, signed_votes: I,) -> Result<FaultLog<N>> where I: IntoIterator<Item = SignedVote<N>>, {
        let mut fault_log = FaultLog::new();
        for signed_vote in signed_votes {
            fault_log.extend(self.add_committed_vote(proposer_id, signed_vote)?);
        }
        Ok(fault_log)
    }

    pub fn add_committed_vote(&mut self, proposer_id: &N, signed_vote: SignedVote<N>,) -> Result<FaultLog<N>> {
        if self.committed.get(&signed_vote.voter).map_or(false, |vote| vote.num >= signed_vote.vote.num) {
            return Ok(FaultLog::new());
        }
        if signed_vote.vote.era != self.era || !self.validate(&signed_vote)? {
            return Ok(FaultLog::init(
                proposer_id.clone(),
                FaultKind::InvalidCommittedVote,
            ));
        }
        self.committed.insert(signed_vote.voter, signed_vote.vote);
        Ok(FaultLog::new())
    }

    pub fn compute_winner(&self) -> Option<&Change<N>> {
        let mut vote_counts: HashMap<&Change<N>, usize> = HashMap::new();
        for vote in self.committed.values() {
            let change = &vote.change;
            let entry = vote_counts.entry(change).or_insert(0);
            *entry += 1;
            if *entry > util::max_faulty(self.pub_keys.len()) {
                return Some(change);
            }
        }
        None
    }

    fn validate(&self, signed_vote: &SignedVote<N>) -> Result<bool> {
        let ser_vote = bincode::serialize(&signed_vote.vote).map_err(|err| Error::SerializeVote(*err))?;
        let pk_opt = self.pub_keys.get(&signed_vote.voter);
        Ok(pk_opt.map_or(false, |pk| pk.verify(&signed_vote.sig, ser_vote)))
    }
}

#[derive(Eq, PartialEq, Debug, Serialize, Deserialize, Hash, Clone)]
struct Vote<N: Ord> {
    change: Change<N>,
    era: u64,
    num: u64,
}

#[derive(Eq, PartialEq, Debug, Serialize, Deserialize, Hash, Clone)]
pub struct SignedVote<N: Ord> {
    vote: Vote<N>,
    voter: N,
    sig: Signature,
}

impl<N: Ord> SignedVote<N> {
    pub fn era(&self) -> u64 {
        self.vote.era
    }

    pub fn voter(&self) -> &N {
        &self.voter
    }
}