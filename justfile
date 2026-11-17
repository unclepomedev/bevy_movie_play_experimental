default:
    @just --list

fix-rs:
    cargo clippy --fix --allow-dirty --allow-staged --all-targets --workspace -- -D warnings
fmt-rs:
    just fix-rs
    cargo fmt --all
fmt: fmt-rs

example id:
    cargo run --example {{ id }}

test:
    cargo test
t: test

# local ci ============================================================================================================

check:
    cargo fmt --all -- --check
    cargo clippy --workspace --all-targets --all-features -- -D warnings
    cargo test --workspace --all-features --lib --tests
    cargo check --examples
    RUSTDOCFLAGS="-D warnings" cargo doc --no-deps
c: check

# experiment ===================
# mkdir -p sandbox/frames
# ffmpeg -f lavfi -i testsrc=duration=15:size=1280x720:rate=30 -pix_fmt yuv420p -c:v libx264 -y sandbox/test.mp4
# ffmpeg -i sandbox/test.mp4 -vf fps=10,scale=640:360 sandbox/frames/%04d.jpg
# cargo run --example sequence_player -- sandbox/frames/ 10
# ffmpeg -i sandbox/test.mp4 -c:v libx264 -pix_fmt yuv420p -bf 0 -g 10 -an sandbox/movie.mp4
# cargo run --example sequence_player --features mp4 -- sandbox/test.mp4
