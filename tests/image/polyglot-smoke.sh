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
node /opt/gascan/tests/playwright-smoke.mjs
git --version
gh --version
cc --version
