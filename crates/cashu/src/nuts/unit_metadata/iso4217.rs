//! ISO 4217 minor units published by SIX on 2026-09-17.
//!
//! Source: <https://www.six-group.com/dam/download/financial-information/data-center/iso-currrency/lists/list-one.xml>.

pub(super) fn precision(unit: &str) -> Option<u8> {
    match unit {
        "bif" | "clp" | "djf" | "gnf" | "isk" | "jpy" | "kmf" | "krw" | "pyg" | "rwf" | "ugx"
        | "uyi" | "vnd" | "vuv" | "xaf" | "xof" | "xpf" => Some(0),
        "aed" | "afn" | "all" | "amd" | "aoa" | "ars" | "aud" | "awg" | "azn" | "bam" | "bbd"
        | "bdt" | "bmd" | "bnd" | "bob" | "bov" | "brl" | "bsd" | "btn" | "bwp" | "byn" | "bzd"
        | "cad" | "cdf" | "che" | "chf" | "chw" | "cny" | "cop" | "cou" | "crc" | "cup" | "cve"
        | "czk" | "dkk" | "dop" | "dzd" | "egp" | "ern" | "etb" | "eur" | "fjd" | "fkp" | "gbp"
        | "gel" | "ghs" | "gip" | "gmd" | "gtq" | "gyd" | "hkd" | "hnl" | "htg" | "huf" | "idr"
        | "ils" | "inr" | "irr" | "jmd" | "kes" | "kgs" | "khr" | "kpw" | "kyd" | "kzt" | "lak"
        | "lbp" | "lkr" | "lrd" | "lsl" | "mad" | "mdl" | "mga" | "mkd" | "mmk" | "mnt" | "mop"
        | "mru" | "mur" | "mvr" | "mwk" | "mxn" | "mxv" | "myr" | "mzn" | "nad" | "ngn" | "nio"
        | "nok" | "npr" | "nzd" | "pab" | "pen" | "pgk" | "php" | "pkr" | "pln" | "qar" | "ron"
        | "rsd" | "rub" | "sar" | "sbd" | "scr" | "sdg" | "sek" | "sgd" | "shp" | "sle" | "sos"
        | "srd" | "ssp" | "stn" | "svc" | "syp" | "szl" | "thb" | "tjs" | "tmt" | "top" | "try"
        | "ttd" | "twd" | "tzs" | "uah" | "usd" | "usn" | "uyu" | "uzs" | "ved" | "ves" | "wst"
        | "xad" | "xcd" | "xcg" | "yer" | "zar" | "zmw" | "zwg" => Some(2),
        "bhd" | "iqd" | "jod" | "kwd" | "lyd" | "omr" | "tnd" => Some(3), // typos: ignore
        "clf" | "uyw" => Some(4),
        _ => None,
    }
}
