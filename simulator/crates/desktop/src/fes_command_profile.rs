//! Raw type-34 commands from each fixed PE's registered ordinary wrappers.
//! These are individual observed values, not a version-wide numeric delta.
//! Evidence: archive/2026-11/20261117_C_fes_raw_command_evidence.json.
//! Native630: archive/2026-11/20261117_C_fes_system_raw_evidence.json.

const CANONICAL: [u32; 14] = [
    337, 338, 339, 340, 341, 342, 346, 347, 348, 349, 350, 353, 358, 357,
];

#[derive(Clone, Copy)]
pub(super) struct FesCommandProfile(&'static [u32], u32);

impl FesCommandProfile {
    pub(super) fn canonical(self, raw: u32) -> Option<u32> {
        if raw == self.1 { return Some(343); }
        self.0.iter().zip(CANONICAL).find_map(|(&observed, canonical)|
            (observed == raw).then_some(canonical))
    }
}

pub(super) fn exact_profile(sha256: &str) -> Option<FesCommandProfile> {
    let (commands, system): (&'static [u32], u32) = match sha256 {
        "0e1dde51e86e9152d8e1d27b82efb15916cb9f82d23dd30990212fafd96c8b9f" => (&[
            326, 327, 328, 329, 330, 331, 335, 336, 337, 338, 339, 342, 347, 346,
        ], 332),
        "1f3288d5fd5408bee4d395faab708aca78c9eeaa6c172dc06dcce09c844ba9d5" => (&[
            337, 338, 339, 340, 341, 342, 346, 347, 348, 349, 350, 353, 358, 357,
        ], 343),
        "3b359fc2d737d5ceed454d6b65e6e5b5e6dfb4a24cf88ee361df86e454203f18" => (&[
            328, 329, 330, 331, 332, 333, 337, 338, 339, 340, 341, 344, 349, 348,
        ], 334),
        "474cc11d875d7be9df9581f3a55d64698ccf4e760c44013df403cced85e505f1" => (&[
            321, 322, 323, 324, 325, 326, 330, 331, 332, 333, 334, 337, 342, 341,
        ], 327),
        "4ccb48ad4bb6bfb5a15a45b8e1f2c2e62072cfad9f1487f869561a473770ed94" => (&[
            331, 332, 333, 334, 335, 336, 340, 341, 342, 343, 344, 347, 352, 351,
        ], 337),
        "63de0e14cd8ce11ad2d68e7ed6076641ff5e5b73d822603cc52764009d812976" => (&[
            337, 338, 339, 340, 341, 342, 346, 347, 348, 349, 350, 353, 358, 357,
        ], 343),
        "6f75ae3877e018de1982a68f1089c127870d895d748c6290dd2ee85cd8daa4de" => (&[
            328, 329, 330, 331, 332, 333, 337, 338, 339, 340, 341, 344, 349, 348,
        ], 334),
        // This older registration table has no native882/883 entries.
        "ac226c8455eff72114d6b1b9929fae01ac49ba90b8ba579f049f179c4c6dd8d5" => (&[
            309, 310, 311, 312, 313, 314, 318, 319, 320, 321, 322, 325,
        ], 315),
        "c2af2ee9baf4b26a3c2cf8a1c867f15f92b54ad1ac5fac381858046a47818f6c" => (&[
            314, 315, 316, 317, 318, 319, 323, 324, 325, 326, 327, 330, 335, 334,
        ], 320),
        "c47c9d883126f641ce07d92eb57b6ac9833fe7495996001c12c68c55b99974d3" => (&[
            323, 324, 325, 326, 327, 328, 332, 333, 334, 335, 336, 339, 344, 343,
        ], 329),
        "c48d383c335c5b283bb0c17bd9f14b2b8bfef4a2e3d635365157b1c69498df78" => (&[
            321, 322, 323, 324, 325, 326, 330, 331, 332, 333, 334, 337, 342, 341,
        ], 327),
        "c71c5b85e6eca660474f156d96b0021a53a5f05690dd077d7861d72ff5b11adc" => (&[
            331, 332, 333, 334, 335, 336, 340, 341, 342, 343, 344, 347, 352, 351,
        ], 337),
        _ => return None,
    };
    Some(FesCommandProfile(commands, system))
}
