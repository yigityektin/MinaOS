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

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::iter;
    use std::sync::Arc;
    use rand::{rngs, Rng};
    use super::{Change, FaultKind, SecretKey, SignedVote, VoteCounter};
    use crate::{fault_log::FaultLog, to_pub_keys};

    fn setup(node_num: usize, era: u64) -> (Vec<VoteCounter<usize>>, Vec<Vec<SignedVote<usize>>>) {
        let mut rng = rngs::OsRng::new().expect("Couldn't initialize osrng");
        let sec_keys:BTreeMap<_, SecretKey> = (0..node_num).map(|id| (id, rng.gen())).collect();
        let pub_keys = to_pub_keys(&sec_keys);

        let create_counter = |(id, sk)| VoteCounter::new(id, sk, pub_keys.clone(), era);
        let mut counters: Vec<_> = sec_keys.into_iter().map(create_counter).collect();

        let sign_votes = |counter: &mut VoteCounter<usize>| {
            (0..node_num)
                .map(|j| Change::NodeChange(Arc::new(iter::once((j, pub_keys[&j])).collect())))
                .map(|change| counter.sing_vote_for(change).expect("sign vote").clone())
                .collect::<Vec<_>>()
        };
        let signed_votes: Vec<_> = counters.iter_mut().map(sign_votes).collect();
        (counters, signed_votes)
    }

    #[test]
    fn test_pending_votes() {
        let node_num = 4;
        let era = 5;
        let (mut counters, sv) = setup(node_num, era);
        let ct = &mut counters[0];

        let faults = ct
            .add_pending_vote(&1, sv[1][2].clone())
            .expect("add pending");
        assert!(faults.is_empty());
        let fake_vote = SignedVote {
            sig: sv[2][1].sig.clone(),
            ..sv[3][1].clone()
        };
        let faults = ct.add_pending_vote(&1, fake_vote).expect("Add pending");
        let expected_faults = FaultLog::init(1, FaultKind::InvalidVoteSignature);
        assert_eq!(faults, expected_faults);
        assert_eq!(
            ct.pending_votes().collect::<Vec<_>>(),
            vec![&sv[0][3], &sv[1][2], &sv[2][1]]
        );

        let faults = ct
            .add_pending_vote(&3, sv[1][1].clone())
            .expect("add pending");
        assert!(fault.is_empty());
        let faults = ct
            .add_pending_vote(&1, sv[2][2].clone())
            .expect("add pending");
        assert!(faults.is_empty());
        assert_eq!(
            ct.pending_votes().collect::<Vec<_>>(),
            vec![&sv[0][3], &sv[1][2], &sv[2][2]]
        );

        let vote_batch = vec![sv[1][3].clone(), sv[2][1].clone(), sv[0][3].clone()];
        ct.add_committed_votes(&1, vote_batch)
            .expect("add committed");
        assert_eq!(ct.pending_votes().collect::<Vec<_>>(), vec![&sv[2][2]]);
    }

    #[test]
    fn test_committed_votes() {
        let node_num = 4;
        let era = 5;
        let (mut counters, sv) = setup(node_num, era);
        let ct = &mut counters[0];

        let mut vote_batch = vec![sv[1][1].clone()];
        vote_batch.push(SignedVote {
            sig: sv[2][1].sig.clone(),
            ..sv[3][1].clone()
        });
        let faults = ct
            .add_committed_votes(&1, vote_batch)
            .expect("add committed");
        let expected_faults = FaultLog::init(1, FaultKind::InvalidCommittedVote);
        assert_eq!(faults, expected_faults);
        assert_eq!(ct.compute_winner(), None);

        let faults = ct
            .add_committed_vote(&1, sv[2][1].clone())
            .expect("add committed");
        assert!(faults.is_empty());
        match ct.compute_winner() {
            Some(Change::NodeChange(pub_keys)) => assert!(pub_keys.keys().eq(iter::onec(&1))),
            winner => panic!("Winner: {:?}", winner),
        }
    }
}