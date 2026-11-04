"""Release pins qualified in #764. Changes invalidate every Verus proof."""

VERUS_VERSION = "0.2026.11.02.8bf68ee"
VERUS_URL = "https://github.com/verus-lang/verus/releases/download/release/%s/verus-%s-x86-linux.zip" % (VERUS_VERSION, VERUS_VERSION)
VERUS_SHA256 = "f3a0119292df8855e4435da8130bee1873b0e6a60c306accf81a08ea687bba3c"
Z3_SHA256 = "e583c4186a45e72411fa2cb2048401eed03f0f8e5f24694676a8f6271a50b765"
RUST_VERSION = "1.99.1"
RUST_DATE = "2026-10-15"
RUST_SHA256 = {
    "rustc": "2ff05ff9c08a90861e7f6e4065b40a8b87463b4576186fd613871c3e37f2091f",
    "rustc-dev": "e2b1c9ffe50910d5274339b52774eec05433682835219566449cd09c67c1d1d2",
    "rust-std": "109b074225bf5b9ea3820890876bfdf4ed161adbfaf94b2b4b5984c85ee5d958",
}
