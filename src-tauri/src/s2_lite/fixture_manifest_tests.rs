use sha2::{Digest, Sha256};

const FIXTURES: &[(&str, &str, &[u8])] = &[
    (
        "activation-cutover-golden-v1.json",
        "64dd36cc26f84fcf7b152b9ef531fb6954e7ca0d035f9fd5a43296f7f1fb02b1",
        include_bytes!("../../../contracts/s2-lite/v1/activation-cutover-golden-v1.json"),
    ),
    (
        "causal-golden-v1.json",
        "7b13c82f6a3abd3be9aea8a028b78bf18bce833099a1ba2991c1e28fb8edca16",
        include_bytes!("../../../contracts/s2-lite/v1/causal-golden-v1.json"),
    ),
    (
        "conflict-golden-v1.json",
        "9187c57e3faf410373738ba5cb282855dc5ef51079dd660672f93a613268de0b",
        include_bytes!("../../../contracts/s2-lite/v1/conflict-golden-v1.json"),
    ),
    (
        "discovery-golden-v1.json",
        "1db1e6accd1a8a52d30b66ae115e893fa1e129adba75854e9115e6d0a2a261e4",
        include_bytes!("../../../contracts/s2-lite/v1/discovery-golden-v1.json"),
    ),
    (
        "float-roundtrip-conflict-v1.jcs",
        "efd47bd91ec9abeadb01bb5da8184720d25df6ecb4890dc0cc9f24bdc02183ad",
        include_bytes!("../../../contracts/s2-lite/v1/float-roundtrip-conflict-v1.jcs"),
    ),
    (
        "jcs-oracle-v1.json",
        "ba8e83b7862ba15aa0ed0312f20d85af3daa35dbcd229ad66a072d8fd8f4051a",
        include_bytes!("../../../contracts/s2-lite/v1/jcs-oracle-v1.json"),
    ),
    (
        "migration-golden-v1.json",
        "23e5fb48388ba8bb1a0e4b33d55b774e752b48b903b4ee03eef17fb8c2e55778",
        include_bytes!("../../../contracts/s2-lite/v1/migration-golden-v1.json"),
    ),
    (
        "ordinary-mutation-semantic-golden-v1.json",
        "625af3376bb412f840e357ac592ee51fd15185e4c6e21c3227d5c18e40d103c2",
        include_bytes!("../../../contracts/s2-lite/v1/ordinary-mutation-semantic-golden-v1.json"),
    ),
    (
        "publish-golden-v1.json",
        "efa8db78aefc0a2ce6e82100ae787271deb85d13d8a7c2b86333b1667ab09e73",
        include_bytes!("../../../contracts/s2-lite/v1/publish-golden-v1.json"),
    ),
    (
        "raw-wire-json-v1.json",
        "72167d12a6f0104fdf489a4d570d6b1c5fa607b7af30b493c456e06c15336ee3",
        include_bytes!("../../../contracts/s2-lite/v1/raw-wire-json-v1.json"),
    ),
];

#[test]
fn frozen_s2_lite_v1_fixture_manifest_matches() {
    for (name, expected, bytes) in FIXTURES {
        assert_eq!(
            format!("{:x}", Sha256::digest(bytes)),
            *expected,
            "fixture drift: {name}"
        );
    }
}
