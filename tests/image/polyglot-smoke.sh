#!/usr/bin/env bash
set -euo pipefail

mise --version
for tool in node python go rust java ruby elixir; do
  mise ls --installed "$tool"
done
node -e 'console.log("node-ok")'
python -c 'print("python-ok")'
printf 'package main\nimport "fmt"\nfunc main(){fmt.Println("go-ok")}\n' >/tmp/main.go
go run /tmp/main.go
printf 'fn main(){println!("rust-ok");}\n' >/tmp/main.rs
rustc /tmp/main.rs -o /tmp/rust-ok
/tmp/rust-ok
printf 'class Main { public static void main(String[] a){ System.out.println("java-ok"); } }\n' >/tmp/Main.java
javac /tmp/Main.java
java -cp /tmp Main
ruby -e 'puts "ruby-ok"'
elixir -e 'IO.puts("elixir-ok")'

resolved=/tmp/gascan-resolved-tool-versions.json
trap 'rm -f "$resolved"' EXIT
jq --null-input \
  --arg elixir "$(mise current elixir)" \
  --arg go "$(mise current go)" \
  --arg java "$(mise current java)" \
  --arg node "$(mise current node)" \
  --arg python "$(mise current python)" \
  --arg ruby "$(mise current ruby)" \
  --arg rust "$(mise current rust)" \
  '$ARGS.named' >"$resolved"
jq --exit-status --slurpfile expected /opt/gascan/image-tool-versions.json \
  '. == $expected[0]' "$resolved" >/dev/null
test "$(stat -c %U:%G /opt/gascan/image-tool-versions.json)" = root:root
test "$(stat -c %a /opt/gascan/image-tool-versions.json)" = 444

node /opt/gascan/tests/playwright-smoke.mjs
git --version
gh --version
cc --version
