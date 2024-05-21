use std::ops::Deref;

use cdk::nuts::{CheckStateRequest, CheckStateResponse, ProofState, State};
use wasm_bindgen::prelude::*;

#[wasm_bindgen(js_name = State)]
pub enum JsState {
    Spent,
    Unspent,
    Pending,
    Reserved,
}

impl From<State> for JsState {
    fn from(inner: State) -> JsState {
        match inner {
            State::Spent => JsState::Spent,
            State::Unspent => JsState::Unspent,
            State::Pending => JsState::Pending,
            State::Reserved => JsState::Reserved,
        }
    }
}

impl From<JsState> for State {
    fn from(inner: JsState) -> State {
        match inner {
            JsState::Spent => State::Spent,
            JsState::Unspent => State::Unspent,
            JsState::Pending => State::Pending,
            JsState::Reserved => State::Reserved,
        }
    }
}

#[wasm_bindgen(js_name = CheckStateRequest)]
pub struct JsCheckStateRequest {
    inner: CheckStateRequest,
}

impl Deref for JsCheckStateRequest {
    type Target = CheckStateRequest;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl From<CheckStateRequest> for JsCheckStateRequest {
    fn from(inner: CheckStateRequest) -> JsCheckStateRequest {
        JsCheckStateRequest { inner }
    }
}

#[wasm_bindgen(js_name = ProofState)]
pub struct JsProofState {
    inner: ProofState,
}

impl Deref for JsProofState {
    type Target = ProofState;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl From<ProofState> for JsProofState {
    fn from(inner: ProofState) -> JsProofState {
        JsProofState { inner }
    }
}

#[wasm_bindgen(js_name = CheckStateResponse)]
pub struct JsCheckStateResponse {
    inner: CheckStateResponse,
}

impl Deref for JsCheckStateResponse {
    type Target = CheckStateResponse;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl From<CheckStateResponse> for JsCheckStateResponse {
    fn from(inner: CheckStateResponse) -> JsCheckStateResponse {
        JsCheckStateResponse { inner }
    }
}
