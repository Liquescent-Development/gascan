use std::io::{self, BufRead, Write};

fn main() -> io::Result<()> {
    let stdin = io::stdin();
    let mut lines = stdin.lock().lines();
    let Some(start) = lines.next().transpose()? else {
        return Ok(());
    };
    if start.contains("bad-version") {
        output(r#"{"version":2,"type":"exit","code":0}"#)?;
        return Ok(());
    }
    if start.contains("no-terminal") {
        return Ok(());
    }
    if start.contains("exit-127") {
        output(r#"{"version":1,"type":"exit","code":127}"#)?;
        return Ok(());
    }

    for line in lines {
        let line = line?;
        if line.contains(r#""type":"stdin""#) {
            output(r#"{"version":1,"type":"stdout","data":"AP8="}"#)?;
        } else if line.contains(r#""type":"resize""#) {
            output(r#"{"version":1,"type":"stdout","data":"NDEgMTEz"}"#)?;
        } else if line.contains(r#""type":"signal""#) {
            output(r#"{"version":1,"type":"exit","code":42}"#)?;
            return Ok(());
        } else if line.contains(r#""type":"close""#) {
            output(r#"{"version":1,"type":"stderr","data":"/gE="}"#)?;
            output(r#"{"version":1,"type":"exit","code":42}"#)?;
            return Ok(());
        }
    }
    Ok(())
}

fn output(frame: &str) -> io::Result<()> {
    let mut stdout = io::stdout().lock();
    writeln!(stdout, "{frame}")?;
    stdout.flush()
}
