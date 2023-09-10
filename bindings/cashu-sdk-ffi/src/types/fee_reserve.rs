use std::{ops::Deref, sync::Arc};

use cashu_ffi::Amount;
use cashu_sdk::mint::FeeReserve as FeeReserveSdk;

pub struct FeeReserve {
    inner: FeeReserveSdk,
}

impl FeeReserve {
    pub fn new(min_fee_reserve: Arc<Amount>, percent_fee_reserve: f32) -> Self {
        Self {
            inner: FeeReserveSdk {
                min_fee_reserve: *min_fee_reserve.as_ref().deref(),
                percent_fee_reserve,
            },
        }
    }
}
