{ pkgs, ... }:
{
  cachix.enable = false;
  packages = with pkgs; [ git cargo rustc rustfmt clippy python3 ];

  tasks."project:doctor".exec = ''
    command -v git cargo rustc
    cargo --version
    rustc --version
    test -n "$DEVENV_ROOT"
    printf 'Environment: %s\n' "$DEVENV_ROOT"
  '';
  tasks."project:test" = {
    after = [ "project:doctor" ];
    exec = "cargo test --workspace --locked";
  };
  tasks."project:verify" = {
    after = [ "project:test" ];
    exec = ''
      cargo fmt --all --check
      cargo clippy --workspace --all-targets --locked -- -D warnings
      python3 scripts/test-guidance-install.py
      python3 scripts/test-release-install.py
    '';
  };
}
