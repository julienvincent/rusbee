use anyhow::Result;
use serde::Serialize;
use tabled::{Table, Tabled, settings::Style};

pub fn render_list<T: Tabled + Serialize>(rows: Vec<T>, empty_msg: &str) -> Result<()> {
  if rows.is_empty() {
    println!("{empty_msg}");
  } else {
    println!("{}", Table::new(rows).with(Style::rounded()));
  }

  Ok(())
}

pub fn render_json<T: Serialize>(value: &T) -> Result<()> {
  println!("{}", serde_json::to_string_pretty(value)?);
  Ok(())
}
