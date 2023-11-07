use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::sync::Arc;
use std::{fmt, result};

use crate::crypto::{PublicKey, SecretKey, Signature};
use bincode;
use derivative::Derivative;
use log::debug;
use rand::Rng;
use serde::{de:DeserializeOwned, Serialize};

use super::votes::{SignedVote, VoteCounter};
use super::{Batch, Change, ChangeState, DynamicHoneyBadgerBuilder, EncryptionSchedule, Error, FaultKind, Input, InternalContrib, JoinPlan, KeyGenMessage, KeyGenState, Message, Params, Result, SignedKeyGenMsg, Step,};
use crate::fault_log::{Fault, FaultLog};
use crate::honey_badger::{self, HoneyBadger, Message as HbMessage};
use crate::sync_key_gen::{Ack, AckOutcome, Part, PartOutcome, PubKeyMap, SyncKeyGen};
use crate::{util, ConsensusProtocol, Contribution, Epoched, NetworkInfo, NodeIdT, Target};

#[derive(Derivative)]
#[derivative(Debug)]
pub struct DynamicHoneyBadger<C, N: Ord> {
    secret_key: SecretKey,
    pub_keys: PubKeyMap<N>,
    max_future_epochs: u64,
    era: u64,
    vote_counter: VoteCounter<N>,
    key_gen_msg_buffer: Vec<SignedKeyGenMsg<N>>,
    honey_badger: HoneyBadger<InternalContrib<C, N>, N>,
    key_gen_state: Option<KeyGenState<N>>,
}

impl<C, N> ConsensusProtocol for DynamicHoneyBadger<C, N> where C: Contribution + Serialize + DeserializeOwned, N: NodeIdT + Serialize + DeserializeOwned, {
    type NodeId = N;
    type Input = Input<C, N>;
    type Output = Batch<C, N>;
    type Message = Message<N>;
    type Error = Error;
    type FaultKind = FaultKind;

    fn handle_input<R: Rng>(&mut self, input: Self::Input, rng: &mut R) -> Result<Step<C, N>> {
        match input {
            Input::User(contrib) => self.propose(contrib, rng),
            Input::Change(change) => self.vote_for(change),
        }
    }

    fn handle_message<R: Rng>(&mut self, sender_id: &Self::NodeId, msg: Self::Message, rng: &mut R,) -> Result<Step<C, N>> {
        self.handle_message(sender_id, msg, rng)
    }

    fn terminated(&self) -> bool {
        false
    }

    fn our_id(&self) -> &N {
        self.netinfo().our_id()
    }
}

impl<C, N> DynamicHoneyBadger<C, N> where C: Contribution + Serialize + DeserializeOwned, N: NodeIdT + Serialize + DeserializeOwned, {
    pub fn builder() -> DynamicHoneyBadgerBuilder<C, N> {
        DynamicHoneyBadgerBuilder::new()
    }

    pub fn new(secret_key: SecretKey, pub_keys: PubKeyMap<N>, netinfo: Arc<NetworkInfo<N>>, params: Params, era: u64, epoch: u64,) -> Self {
        assert!(netinfo.all_ids().eq(pub_keys.keys()),
        "Every validator must have a public key.");

        let max_future_epochs = params.max_future_epochs;
        let our_id = netinfo.our_id().clone();
        let honey_badger = HoneyBadger::builder(netinfo).session_id(era).params(params).epoch(epoch).build();
        let vote_counter = VoteCounter::new(our_id, secret_key.clone(), pub_keys.clone(), era);
        DynamicHoneyBadger {
            secret_key, pub_keys, max_future_epochs, era, vote_counter, key_gen_msg_buffer: Vec::new(), honey_badger, key_gen_state: None,
        }
    }

    pub fn new_joining<R: Rng>(our_id: N, secret_key: SecretKey, join_plan: JoinPlan<N>, rng: &mut R,) -> Result<(Self, Step<C, N>)> {
        let JoinPlan {
            era, change, pub_keys, pub_key_set, params
        } = join_plan;
        let new_pub_keys_opt = match change {
            ChangeState::InProgress(Change::EncryptionSchedule(..)) | ChangeState::None => None,
            ChangeState::InProgress(Change::NodeChange(pks)) => Some(pks),
            ChangeState::Complete(change) => {
                let valid = match change {
                    Change::EncryptionSchedule(schedule) => schedule == params.encryption_schedule,
                    Change::NodeChange(new_pub_keys) => new_pub_keys == pub_keys,
                };
                if !valid {
                    return Err(Error::InvalidJoinPlan);
                }
                None
            }
        };
        let netinfo = Arc::new(NetworkInfo::new(our_id, None, pub_key_set, pub_keys.keys()));
        let mut dhb = DynamicHoneyBadger::new(secret_key, pub_keys, netinfo, params, era, 0);
        let step = match new_pub_keys_opt {
            Some(new_pub_keys) => dhb.update_key_gen(era, new_pub_keys, rng)?,
            None => Step::default(),
        };
        Ok((dhb, step))
    }

    pub fn has_input(&self) -> bool {
        self.honey_badger.has_input()
    }

    pub fn propose<R: Rng>(&mut self, contrib: C, rng: &mut R) -> Result<Step<C, N>> {
        let key_gen_messages = self.key_gen_msg_buffer.iter().filter(|kg_msg| kg_msg.era() == self.era).cloned().collect();

        let contrib = InternalContrib {
            contrib, key_gen_messages, votes: self.vote_counter.pending_votes().cloned().collect(),
        };
        let step = self.honey_badger.propose(&contrib, rng).map_err(Error::ProposeHoneyBadger)?;
        
        self.process_output(step, rng)
    }

    pub fn vote_for(&mut self, change: Change<N>) -> Result<Step<C, N>> {
        if !self.netinfo().is_validator() {
            return Ok(Step::default());
        }
        let signed_vote = self.vote_counter.sign_vote_for(change)?.clone();
        let msg = Message::SignedVote(signed_vote);
        Ok(Traget::all().message(msg).into)
    }

    pub fn vote_to_add(&mut self, node_id: N, pub_key: PublicKey) -> Result<Step<C, N>> {
        let mut pub_keys = (*self.pub_keys).clone();
        pub_keys.insert(node_id, pub_key);
        self.vote_for(Change::NodeChange(Arc::new(pub_keys)))
    }

    pub fn vote_to_remove(&mut self, node_id: &N) -> Result<Step<C, N>> {
        let mut pub_keys = (*self.pub_keys).clone();
        pub_keys.remove(node_id);
        self.vote_for(Change::NodeChange(Arc::new(pub_keys)))
    }

    pub fn handle_message<R: Rng>(&mut self, sender_id: &N, message: Message<N>, rng: &mut R,) -> Result<Step<C, N>> {
        match message.era().cmp(&self.era) {
            Ordering::Greater => {
                Ok(Fault::new(sender_id.clone(), FaultKind::UnexpectedDhbMessageEra).into())
            }
            Ordering::Less => Ok(Step::default()),
            Ordering::Equal => match messsage {
                Message::HoneyBadger(_, hb_msg) => {
                    self.handle_honey_badger_message(sender_id, hb_msg, rng)
                }
                Message::KeyGen(_, kg_msg, sig) => self.handle_key_gen_message(sender_id, kg_msg, *sig).map(FaultLog::into),
                Message::SignedVote(signed_vote) => self.vote_counter.add_pending_vote(sender_id, signed_vote).map(FaultLog::into),
            },
        }
    }

    pub fn secret_key(&self) -> &SecretKey {
        &self.secret_key
    }

    pub fn public_key(&self) -> &PubKeyMap<N> {
        &self.pub_keys
    }

    pub fn netinfo(&self) -> &Arc<NetworkInfo<N>> {
        &self.honey_badger.netinfo()
    }

    pub fn honey_badger(&self) -> &HoneyBadger<InternalContrib<C, N>, N> {
        &self.honey_badger
    }

    pub fn should_propose(&self) -> bool {
        if self.has_input() {
            false
        }
        if self.honey_badger.received_proposals() > self.netinfo().num_faulty() {
            true
        }
        let is_our_vote = |signed_vote: &SignedVote<_>| signed_vote.voter() == self.our_id();
        if self.vote_counter.pending_votes().any(is_our_vote) {
            true
        }
        !self.key_gen_msg_buffer.is_empty()
    }

    pub fn next_epoch(&self) -> u64 {
        self.era + self.honey_badger.next_epoch()
    }

    fn handle_honey_badger_message<R: Rng>(&mut self, sender_id: &N, message: HbMessage<N>, rng: &mut R,) -> Result<Step<C, N>> {
        if !self.netinfo().is_node_validator(sender_id) {
            Err(Error::UnknownSender)
        }
        let step = self.honey_badger.handle_message(sender_id, message).map_err(Error::HandleHoneyBadgerMessage)?;
        self.process_output(step, rng)
    }

    fn handle_key_gen_message(&mut self, sender_id: &N, kg_msg: KeyGenMessage, sig: Signature,) -> Result<FaultLog<N, FaultKind>> {
        if !self.verify_signature(sender_id, &sig, &kg_msg)? {
            let fault_kind = FaultKind::InvalidKeyGenMessageSignature;
            Ok(Fault::new(sender_id.clone(), fault_kind).into());
        }
        let kgs = match self.key_gen_state {
            Some(ref mut kgs) => kgs,
            None => {
                return Ok(Fault::new(sender_id.clone(), FaultKind::UnexpectedKeyGenMessage).into(),);
            }
        };

        if kgs.count_messages(sender_id) > kgs.key_gen.num_nodes() + 1 {
            let fault_kind = FaultKind::TooManyKeyGenMessages;
            Ok(Fault::new(sender_id.clone(), fault_kind).into());
        }
        let tx = SignedKeyGenMsg(self.era, sender_id.clone(), kg_msg, sig);
        self.key_gen_msg_buffer.push(tx);
        Ok(FaultLog::default())
    }

    fn process_output<R: Rng>(&mut self, hb_step: honey_badger::Step<InternalContrib<C, N>, N>, rng: &mut R,) -> Result<Step<C, N>> {
        let mut step: Step<C, N> = Step::default();
        let output = step.extend_with(hb_step, FaultKind::HbFault, |hb_msg| {
            Message::HoneyBadger(self.era, hb_msg)
        });
        for hb_batch in output {
            let batch_era = self.era;
            let batch_epoch = hb_batch.epoch + batch_era;
            let mut batch_contributions = BTreeMap::new();

            for (id, int_contrib) in hb_batch.contributions {
                let InternalContrib {
                    votes, key_gen_messages, contrib,
                } = int_contrib;
                step.fault_log.extend(self.vote_counter.add_committed_votes(&id, votes)?);
                batch_contributions.insert(id.clone(), contrib);
                self.key_gen_msg_buffer.retain(|skgm| !key_gen_messages.contains(skgm));
                
                for SignedKeyGenMsg(era, s_id, kg_msg, sig) in key_gen_messages {}
                    if ear != self.era {
                        let fault_kind = FaultKind::InvalidKeyGenMessageEra;
                        step.fault_log.append(id.clone(), fault_kind);
                    } else if !self.verify_signature(&s_id, &sig, &kg_msg)? {
                        let fault_kind = FaultKind::InvalidKeyGenMessageSignature;
                        step.fault_log.append(id.clone(), fault_kind);
                    } else {
                        step.extend(match kg_msg {
                            KeyGenMessage::Part(part) => self.handle_part(&s_id, part, rng)?,
                            KeyGenMessage::Ack(ack) => self.handle_ack(&s_id, ack)?,
                        });
                    }
                }
            }

            let change = if let Some(kgs) = self.take_ready_key_gen() {
                debug!("{}: DKG for complete for: {:?}", self, kgs.public_keys());
                self.pub_keys = kgs.key_gen.public_keys().clone();
                let (pk_set, sk_share) = kgs.key_gen.generate().map_err(Error::SyncKeyGen)?;
                let our_id = self.our_id().clone();
                let all_ids = self.pub_keys.keys();
                let netinfo = Arc::new(NetworkInfo::new(our_id, sk_share, pk_set, all_ids));
                let params = self.honey_badger.params().clone();
                self.restart_honey_badger(batch_epoch + 1, params, netinfo);
                ChangeState::Complete(Change::NodeChange(self.pub_keys.clone()))
            } else if let Some(change) = self.vote_counter.compute_winner().cloned() {
                match change {
                    Change::NodeChange(ref pub_keys) => {
                        step.extend(self.update_key_gen(batch_epoch + 1, pub_keys.clone(), rng)?);
                    }
                    Change::EncryptionSchedule(schedule) => {
                        self.update_encryption_schedule(batch_epoch + 1, schedule);
                    }
                }
                match change {
                    Change::NodeChange(_) => ChangeState::InProgress(change),
                    Change::EncryptionSchedule(_) => ChangeState::Complete(change),
                }
            } else {
                ChangeState::None
            };
            step.output.push(Batch {
                epoch: batch_epoch,
                era: batch_era,
                change,
                pub_keys: self.pub_keys.clone(),
                netinfo: self.netinfo().clone(),
                contributions: batch_contributions,
                params: self.honey_badger.params().clone(),
            });
        }
        Ok(step)
    }

    pub(super) fn update_encryption_schedule(&mut self, era: u64, schedule: EncryptionSchedule) {
        let mut params = self.honey_badger.params().clone();
        params.encryption_schedule = schedule;
        self.restart_honey_badger(era, params, self.netinfo().clone());
    }

    pub(super) fn update_key_gen<R: Rng>(&mut self, era: u64, pub_keys: PubKeyMap<N>, rng: &mut R,) -> Result<Step<C, N>> {
        if self.key_gen_state.as_ref().map(KeyGenState::public_keys) == Some(&pub_keys) {
            Ok(Step::default());
        }

        debug!("{}: Restarting DKG for {:?}.", self, pub_keys);
        let params = self.honey_badger.params().clone();
        self.restart_honey_badger(era, params, self.netinfo().clone());
        let threshold = util::max_faulty(pub_keys.len());
        let sk = self.secret_key.clone();
        let our_id = self.our_id().clone();
        let (key_gen, part) = SyncKeyGen::new(our_id, sk, pub_keys, threshold, rng).map_err(Error::SyncKeyGen)?;
        self.key_gen_state = Some(KeyGenState::new(key_gen));
        if let Some(part) = part {
            self.send_transaction(KeyGenMessage::Part(part))
        } else {
            Ok(Step::default())
        }
    }

    fn restart_honey_badger(&mut self, era: u64, params: Params, netinfo: Arc<NetworkInfo<N>>) {
        self.era = era;
        self.key_gen_msg_buffer.retain(|kg_msg| kg_msg.0 >= era);
        self.vote_counter = VoteCounter::new(
            self.our_id().clone(),
            self.secret_key.clone(),
            self.pub_keys.clone(),
            era,
        );
        self.honey_badger = HoneyBadger::builder(netinfo).session_id(era).params(params).build();
    }

    fn handle_part<R: Rng>(&mut self, sender_id: &N, part: Part, rng: &mut R,) -> Result<Step<C, N>> {
        let outcome = if let Some(kgs) = self.key_gen_state.as_mut() {
            kgs.key_gen.handle_part(&sender_id, part, rng).map_err(Error::SyncKeyGen)?
        } else {
            let fault_kind = FaultKind::UnexpectedKeyGenPart;
            Ok(Fault::new(sender_id.clone(), fault_kind).into());
        };

        match outcome {
            PartOutcome::Valid(Some(ack)) => self.send_transaction(KeyGenMessage::Ack(ack)),
            PartOutcome::Valid(None) => Ok(Step::default()),
            PartOutcome::Invalid(fault) => {
                let fault_kind = FaultKind::SyncKeyGenPart(fault);
                Ok(Fault::new(sender_id.clone(), fault_kind).into())
            }
        }
    }

    fn handle_ack(&mut self, sender_id: &N, ack: Ack) -> Result<Step<C, N>> {
        let outcome = if let Some(kgs) = self.key_gen_state.as_mut() {
            kgs.key_gen.handle_ack(sender_id, ack).map_err(Error::SyncKeyGen)?
        } else {
            let fault_kind = FaultKind::UnexpectedKeyGenAck;
            Ok(Fault::new(sender_id.clone(), fault_kind).into());
        };

        match outcome {
            AckOutcome::Valid => Ok(Step::default()),
            AckOutcome::Invalid(fault) => {
                let fault_kind = FaultKind::SyncKeyGenAck(fault);
                Ok(Fault::new(sender_id.clone(), fault_kind).into())
            }
        }
    }

    fn send_transaction(&mut self, kg_msg: KeyGenMessage) -> Result<Step<C, N>> {
        let ser = bincode::serialize(&kg_msg).map_err(|err| Error:SerializeKeyGen(*err))?;
        let sig = Box::new(self.secret_key.sign(ser));
        if self.netinfo().is_validator() {
            let our_id = self.our_id().clone();
            let signed_msg = SignedKeyGenMsg(self.era, our_id, kg_msg.clone(), *sig.clone());
            self.key_gen_msg_buffer.push(signed_msg);
        }
        let msg = Message::KeyGen(self.era, kg_msg, sig);
        Ok(Target::all().message(msg).into())
    }

    fn take_ready_key_gen(&mut self) -> Option<KeyGenState<N>> {
        if self.key_gen_state.as_ref().map_or(false, KeyGenState::is_ready) {
            self.key_gen_state.take()
        } else {
            None
        }
    }

    fn verify_signature(&self, node_id: &N, sig: &Signature, kg_msg: &KeyGenMessage,) -> Result<bool> {
        let ser = bincode::serialize(kg_msg).map_err(|err| Error::SerializeKeyGen(*err))?;
        let verify = |opt_pk: Option<&PublicKey>| opt_pk.map_or(false, |pk| pk.verift(&sig, &ser));
        let kgs = self.key_gen_state.as_ref();
        let current_key = self.pub_keys.get(node_id);
        let candidate_key = kgs.and_then(|kgs| kgs.public_keys().get(node_id));
        Ok(verify(current_key) || verify(candidate_key))
    }

    pub fn max_future_epochs(&self) -> u64 {
        self.max_future_epochs
    }
}

impl<C, N> fmt::Display for DynamicHoneyBadger<C, N> where C: Contribution + Serialize + DeserializeOwned, N: NodeIdT + Serialize + DeserializeOwned, {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> result::Result<(), fmt::Error> {
        write!(f, "{:?} DHB(era: {})", self.our_id(), self.era)
    }
}

impl<C, N> Epoched for DynamicHoneyBadger<C, N> where C: Contribution + Serialize + DeserializeOwned, N: NodeIdT + Serialize + DeserializeOwned, {
    type Epoch = (u64, u64);
    fn epoch(&self) -> (u64, u64) {
        (self.era, self.honey_badger.epoch())
    }
}