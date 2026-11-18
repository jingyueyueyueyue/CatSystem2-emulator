//! Exact runtime task-factory local facts. These are proven original null
//! branches, never fallback results for an unimplemented constructor.
const V401: &str = "0e1dde51e86e9152d8e1d27b82efb15916cb9f82d23dd30990212fafd96c8b9f";
const YUKIKOI: &str = "63de0e14cd8ce11ad2d68e7ed6076641ff5e5b73d822603cc52764009d812976";
const NEWTON_BASE: &str = "3b359fc2d737d5ceed454d6b65e6e5b5e6dfb4a24cf88ee361df86e454203f18";
const NEWTON_LOCAL: &str = "6f75ae3877e018de1982a68f1089c127870d895d748c6290dd2ee85cd8daa4de";

// Top RTTI -> type-one route -> native66 factory -> direct kind43 slot.
// Each tuple is (PE SHA256, factory table RVA, null branch RVA); the full
// prefix, route and epilogue bytes are retained in
// reports/css_task_576_F_contracts_20261117.json. No kind14 result is inferred
// for these additional images, and this is not a constant576 declaration.
const KIND43_NULL: &[(&str, u32, u32)] = &[
    ("f6af7437475b8c066be973245aa79688bea510ca1192264add4f1914ebee4c0e", 0x019bc4, 0x019bb9),
    ("012c71635932c241cde4df986377516279a50b907166757e89ea7b86ff3f813f", 0x019c44, 0x019c39),
    ("9f3a5ad0ed22074420685f1e2a3bc92436d68289c7bac854d5f7d7331dc3ef67", 0x019aa4, 0x019a99),
    (V401, 0x0baa90, 0x0baa86),
    ("c71c5b85e6eca660474f156d96b0021a53a5f05690dd077d7861d72ff5b11adc", 0x1de9a4, 0x1de989),
    ("474cc11d875d7be9df9581f3a55d64698ccf4e760c44013df403cced85e505f1", 0x0a7cec, 0x0a7ce0),
    (YUKIKOI, 0x136428, 0x136412),
    ("c47c9d883126f641ce07d92eb57b6ac9833fe7495996001c12c68c55b99974d3", 0x0aef2c, 0x0aef20),
    ("4ccb48ad4bb6bfb5a15a45b8e1f2c2e62072cfad9f1487f869561a473770ed94", 0x1a2d9c, 0x1a2d83),
    ("c48d383c335c5b283bb0c17bd9f14b2b8bfef4a2e3d635365157b1c69498df78", 0x0a7cec, 0x0a7ce0),
    ("c2af2ee9baf4b26a3c2cf8a1c867f15f92b54ad1ac5fac381858046a47818f6c", 0x05c674, 0x05c668),
    ("ac226c8455eff72114d6b1b9929fae01ac49ba90b8ba579f049f179c4c6dd8d5", 0x0365ec, 0x0365e0),
    (NEWTON_BASE, 0x1949f0, 0x1949e4),
    (NEWTON_LOCAL, 0x1949f0, 0x1949e4),
    ("1f3288d5fd5408bee4d395faab708aca78c9eeaa6c172dc06dcce09c844ba9d5", 0x1359e8, 0x1359d2),
];

/// None requires the real constructor path. Some(0) means this identified
/// original factory itself returned no object for the requested kind.
pub fn exact_factory_result(pe_sha256: &str, kind: u32) -> Option<u32> {
    if kind == 43 && KIND43_NULL.iter().any(|entry| entry.0 == pe_sha256) {
        return Some(0);
    }
    match (pe_sha256, kind) {
        // Factory004b9f40, table004baa90, cases14/43 ->004baa86:
        // three pops; xor eax,eax; two pops; ret0xc. No constructor/callback.
        (V401, 14 | 43) => Some(0),
        // Table00536428, direct cases ->00536412, zero EAX followed
        // exclusively by the frame/SEH restoration and ret0xc.
        (YUKIKOI, 14 | 43) => Some(0),
        // Both independently fingerprinted Newton images: table005949f0,
        // cases14/43 ->005949e4. Pure register restoration/zero/ret0xc.
        (NEWTON_BASE | NEWTON_LOCAL, 14 | 43) => Some(0),
        _ => None,
    }
}
