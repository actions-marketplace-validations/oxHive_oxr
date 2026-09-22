import 'recipes/release.just'

_default:
  @just --choose

run cmd:
  cargo run -- {{cmd}}

run-init:
  just run init

release-major:
  just _release major

release-minor:
  just _release minor

release-patch:
  just _release patch

