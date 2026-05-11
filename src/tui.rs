use std::{
  io,
  sync::Arc,
  time::{Duration, Instant},
};

use anyhow::Result;
use crossterm::{
  event::{self, Event, KeyCode},
  execute,
  terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{
  Terminal,
  backend::CrosstermBackend,
  layout::{Constraint, Direction, Layout, Rect},
  style::{Color, Modifier, Style},
  text::{Line, Span},
  widgets::{Block, Borders, Cell, Clear, Paragraph, Row, Table, TableState},
};
use tokio::task::JoinHandle;

use crate::{
  client::Client,
  device::{self, UsbDevice},
  usbip::Usbip,
};

const REFRESH_INTERVAL: Duration = Duration::from_secs(2);
const NOTIFICATION_TTL: Duration = Duration::from_secs(5);

pub async fn run<C>(client: C, usbip: Arc<dyn Usbip>, remote_host: String) -> Result<()>
where
  C: Client + Clone + Send + Sync + 'static,
{
  enable_raw_mode()?;
  let mut stdout = io::stdout();
  execute!(stdout, EnterAlternateScreen)?;

  let backend = CrosstermBackend::new(stdout);
  let mut terminal = Terminal::new(backend)?;
  let result = run_app(&mut terminal, client, usbip, remote_host).await;

  disable_raw_mode()?;
  execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
  terminal.show_cursor()?;

  result
}

async fn run_app<C>(
  terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
  client: C,
  usbip: Arc<dyn Usbip>,
  remote_host: String,
) -> Result<()>
where
  C: Client + Clone + Send + Sync + 'static,
{
  let mut devices = Vec::new();
  let mut state = TableState::default().with_selected(Some(0));
  let mut last_refresh = Instant::now() - REFRESH_INTERVAL;
  let mut refresh_task: Option<JoinHandle<Result<Vec<UsbDevice>>>> = None;
  let mut action_task: Option<JoinHandle<ActionResult>> = None;
  let mut notification: Option<Notification> = None;
  let mut show_details = false;

  loop {
    if notification
      .as_ref()
      .is_some_and(|notification| notification.created_at.elapsed() >= NOTIFICATION_TTL)
    {
      notification = None;
    }

    if refresh_task.as_ref().is_some_and(|task| task.is_finished()) {
      let task = refresh_task.take().expect("finished refresh task exists");
      match task.await? {
        Ok(next_devices) => {
          devices = next_devices;
          if devices.is_empty() {
            state.select(None);
          } else if state.selected().is_none_or(|index| index >= devices.len()) {
            state.select(Some(0));
          }
        }
        Err(error) => notification = Some(Notification::error(error.to_string())),
      }
    }

    if action_task.as_ref().is_some_and(|task| task.is_finished()) {
      let task = action_task.take().expect("finished action task exists");
      match task.await? {
        Ok(message) => {
          notification = Some(Notification::success(message));
        }
        Err(error) => notification = Some(Notification::error(error.to_string())),
      }

      if refresh_task.is_none() {
        refresh_task = Some(spawn_refresh(client.clone(), usbip.clone()));
        last_refresh = Instant::now();
      } else {
        last_refresh = Instant::now() - REFRESH_INTERVAL;
      }
    }

    if last_refresh.elapsed() >= REFRESH_INTERVAL && refresh_task.is_none() {
      refresh_task = Some(spawn_refresh(client.clone(), usbip.clone()));
      last_refresh = Instant::now();
    }

    let action_busy = action_task.is_some();
    terminal.draw(|frame| {
      draw(
        frame,
        &mut state,
        &devices,
        notification.as_ref(),
        action_busy,
        show_details,
      )
    })?;

    if !event::poll(Duration::from_millis(50))? {
      continue;
    }

    let Event::Key(key) = event::read()? else {
      continue;
    };

    match key.code {
      KeyCode::Char('q') => return Ok(()),
      KeyCode::Char('r') => {
        if refresh_task.is_none() {
          refresh_task = Some(spawn_refresh(client.clone(), usbip.clone()));
          last_refresh = Instant::now();
        }
      }
      KeyCode::Down | KeyCode::Char('j') => select_next(&mut state, devices.len()),
      KeyCode::Up | KeyCode::Char('k') => select_previous(&mut state, devices.len()),
      KeyCode::Esc => show_details = false,
      KeyCode::Char('i') => show_details = !show_details,
      KeyCode::Char('b') => {
        if action_task.is_none()
          && let Some(device) = selected_device(&devices, &state)
        {
          if device.bound {
            notification = Some(Notification::warning("selected device is already bound"));
          } else {
            action_task = Some(bind_action(client.clone(), device));
          }
        }
      }
      KeyCode::Char('u') => {
        if action_task.is_none()
          && let Some(device) = selected_device(&devices, &state)
        {
          action_task = Some(unbind_action(client.clone(), device));
        }
      }
      KeyCode::Char('a') => {
        if action_task.is_none()
          && let Some(device) = selected_device(&devices, &state)
        {
          action_task = Some(attach_action(
            client.clone(),
            usbip.clone(),
            remote_host.clone(),
            device,
          ));
        }
      }
      KeyCode::Char('d') => {
        if action_task.is_none()
          && let Some(device) = selected_device(&devices, &state)
        {
          if let Some(port) = device.attached_port {
            action_task = Some(detach_action(usbip.clone(), port));
          } else {
            notification = Some(Notification::warning("selected device is not attached"));
          }
        }
      }
      _ => {}
    }
  }
}

type ActionResult = Result<String>;

struct Notification {
  message: String,
  kind: NotificationKind,
  created_at: Instant,
}

impl Notification {
  fn success(message: impl Into<String>) -> Self {
    Self::new(message, NotificationKind::Success)
  }

  fn warning(message: impl Into<String>) -> Self {
    Self::new(message, NotificationKind::Warning)
  }

  fn error(message: impl Into<String>) -> Self {
    Self::new(message, NotificationKind::Error)
  }

  fn new(message: impl Into<String>, kind: NotificationKind) -> Self {
    Self {
      message: message.into(),
      kind,
      created_at: Instant::now(),
    }
  }
}

enum NotificationKind {
  Success,
  Warning,
  Error,
}

fn spawn_refresh<C>(client: C, usbip: Arc<dyn Usbip>) -> JoinHandle<Result<Vec<UsbDevice>>>
where
  C: Client + Send + Sync + 'static,
{
  tokio::spawn(async move {
    let mut devices = client.list_devices().await?;
    let attachments = usbip.list_attachments().await?;

    device::mark_attached(&mut devices, &attachments);

    Ok(devices)
  })
}

fn bind_action<C>(client: C, device: &UsbDevice) -> JoinHandle<ActionResult>
where
  C: Client + Send + Sync + 'static,
{
  let busid = device.busid.clone();
  let name = device.name.clone();
  tokio::spawn(async move {
    client.bind_device(&busid).await?;
    Ok(format!("bound {busid} ({name})"))
  })
}

fn unbind_action<C>(client: C, device: &UsbDevice) -> JoinHandle<ActionResult>
where
  C: Client + Send + Sync + 'static,
{
  let busid = device.busid.clone();
  let name = device.name.clone();
  tokio::spawn(async move {
    client.unbind_device(&busid).await?;
    Ok(format!("unbound {busid} ({name})"))
  })
}

fn attach_action<C>(
  client: C,
  usbip: Arc<dyn Usbip>,
  remote_host: String,
  device: &UsbDevice,
) -> JoinHandle<ActionResult>
where
  C: Client + Send + Sync + 'static,
{
  let busid = device.busid.clone();
  let name = device.name.clone();
  tokio::spawn(async move {
    let device = client
      .list_devices()
      .await?
      .into_iter()
      .find(|device| device.busid == busid)
      .ok_or_else(|| anyhow::anyhow!("device {busid} was not found"))?;

    if !device.bound {
      client.bind_device(&busid).await?;
    }

    usbip.attach(&remote_host, &busid).await?;
    Ok(format!("attached {busid} ({name})"))
  })
}

fn detach_action(usbip: Arc<dyn Usbip>, port: u16) -> JoinHandle<ActionResult> {
  tokio::spawn(async move {
    usbip.detach(port).await?;
    Ok(format!("detached port {port}"))
  })
}

fn draw(
  frame: &mut ratatui::Frame,
  state: &mut TableState,
  devices: &[UsbDevice],
  notification: Option<&Notification>,
  busy: bool,
  show_details: bool,
) {
  let areas = Layout::default()
    .direction(Direction::Vertical)
    .constraints([Constraint::Min(3), Constraint::Length(3)])
    .split(frame.area());

  let rows = devices.iter().map(|device| {
    let usbid = if device.vendor_id.is_empty() || device.product_id.is_empty() {
      "unknown".to_string()
    } else {
      format!("{}:{}", device.vendor_id, device.product_id)
    };

    Row::new([
      Cell::from(device.busid.clone()).style(Style::default().fg(Color::Cyan)),
      Cell::from(usbid),
      bool_cell(device.bound),
      bool_cell(device.attached),
      Cell::from(display_value(&device.metadata.manufacturer)),
      Cell::from(display_value(&device.metadata.product)),
    ])
  });

  let table = Table::new(
    rows,
    [
      Constraint::Length(10),
      Constraint::Length(11),
      Constraint::Length(8),
      Constraint::Length(10),
      Constraint::Length(24),
      Constraint::Min(24),
    ],
  )
  .header(
    Row::new([
      "BUSID",
      "USBID",
      "BOUND",
      "ATTACHED",
      "MANUFACTURER",
      "PRODUCT",
    ])
    .style(
      Style::default()
        .fg(Color::Yellow)
        .add_modifier(Modifier::BOLD),
    )
    .bottom_margin(1),
  )
  .block(
    Block::default()
      .title(" rusbee devices ")
      .title_style(
        Style::default()
          .fg(Color::Magenta)
          .add_modifier(Modifier::BOLD),
      )
      .borders(Borders::ALL)
      .border_style(Style::default().fg(Color::Blue)),
  )
  .row_highlight_style(Style::default().add_modifier(Modifier::BOLD))
  .highlight_symbol(Line::from(Span::styled(
    "❯ ",
    Style::default().fg(Color::Yellow),
  )));

  frame.render_stateful_widget(table, areas[0], state);

  let status = Line::from(
    "q quit | r refresh | j/k move | i details | b bind | u unbind | a attach | d detach",
  );
  let status_border = if busy { Color::Yellow } else { Color::DarkGray };

  frame.render_widget(
    Paragraph::new(status).block(
      Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(status_border)),
    ),
    areas[1],
  );

  if show_details && let Some(device) = selected_device(devices, state) {
    draw_details(frame, device);
  }

  if let Some(notification) = notification {
    draw_notification(frame, notification);
  }
}

fn display_value(value: &str) -> String {
  if value.is_empty() {
    "unknown".to_string()
  } else {
    value.to_string()
  }
}

fn draw_details(frame: &mut ratatui::Frame, device: &UsbDevice) {
  let area = centered_rect(frame.area(), 74, 20);
  let lines = vec![
    detail_line("BUSID", &device.busid),
    detail_line(
      "USBID",
      &format!("{}:{}", device.vendor_id, device.product_id),
    ),
    detail_line(
      "Manufacturer",
      display_value(&device.metadata.manufacturer).as_str(),
    ),
    detail_line("Product", display_value(&device.metadata.product).as_str()),
    detail_line("Name", &device.name),
    detail_line("Bound", if device.bound { "yes" } else { "no" }),
    detail_line("Attached", if device.attached { "yes" } else { "no" }),
    detail_line("Attached port", optional_u16(device.attached_port).as_str()),
    detail_line(
      "Serial",
      optional_value(device.metadata.serial.as_deref()).as_str(),
    ),
    detail_line(
      "Speed",
      optional_value(device.metadata.speed.as_deref()).as_str(),
    ),
    detail_line(
      "USB version",
      optional_value(device.metadata.usb_version.as_deref()).as_str(),
    ),
    detail_line(
      "Device version",
      optional_value(device.metadata.device_version.as_deref()).as_str(),
    ),
    detail_line(
      "Bus number",
      optional_value(device.metadata.busnum.as_deref()).as_str(),
    ),
    detail_line(
      "Device number",
      optional_value(device.metadata.devnum.as_deref()).as_str(),
    ),
    detail_line(
      "Device class",
      optional_value(device.metadata.device_class.as_deref()).as_str(),
    ),
    detail_line(
      "Device subclass",
      optional_value(device.metadata.device_subclass.as_deref()).as_str(),
    ),
    detail_line(
      "Device protocol",
      optional_value(device.metadata.device_protocol.as_deref()).as_str(),
    ),
    detail_line(
      "Driver",
      optional_value(device.metadata.driver.as_deref()).as_str(),
    ),
  ];

  let widget = Paragraph::new(lines).block(
    Block::default()
      .title(" device details ")
      .title_style(
        Style::default()
          .fg(Color::Magenta)
          .add_modifier(Modifier::BOLD),
      )
      .borders(Borders::ALL)
      .border_style(Style::default().fg(Color::Magenta)),
  );

  frame.render_widget(Clear, area);
  frame.render_widget(widget, area);
}

fn detail_line(label: &str, value: &str) -> Line<'static> {
  Line::from(format!("{label:<16} {value}"))
}

fn optional_value(value: Option<&str>) -> String {
  value.unwrap_or("unknown").to_string()
}

fn optional_u16(value: Option<u16>) -> String {
  value
    .map(|value| value.to_string())
    .unwrap_or_else(|| "unknown".to_string())
}

fn centered_rect(area: Rect, width: u16, height: u16) -> Rect {
  let width = width.min(area.width.saturating_sub(2));
  let height = height.min(area.height.saturating_sub(2));
  let x = area.x + area.width.saturating_sub(width) / 2;
  let y = area.y + area.height.saturating_sub(height) / 2;

  Rect::new(x, y, width, height)
}

fn bool_cell(value: bool) -> Cell<'static> {
  let (text, color) = if value {
    ("yes", Color::Green)
  } else {
    ("no", Color::DarkGray)
  };

  Cell::from(text).style(Style::default().fg(color).add_modifier(Modifier::BOLD))
}

fn draw_notification(frame: &mut ratatui::Frame, notification: &Notification) {
  let area = notification_area(frame.area(), &notification.message);
  let lines = notification_lines(&notification.message, area);
  let (title, color) = match notification.kind {
    NotificationKind::Success => (" success ", Color::Green),
    NotificationKind::Warning => (" notice ", Color::Yellow),
    NotificationKind::Error => (" error ", Color::Red),
  };

  let widget = Paragraph::new(lines)
    .style(Style::default().fg(color))
    .block(
      Block::default()
        .title(title)
        .title_style(Style::default().fg(color).add_modifier(Modifier::BOLD))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(color)),
    );

  frame.render_widget(Clear, area);
  frame.render_widget(widget, area);
}

fn notification_area(area: Rect, message: &str) -> Rect {
  let width = area.width.saturating_sub(2).min(64).max(24);
  let content_width = width.saturating_sub(2).max(1) as usize;
  let line_count = message
    .lines()
    .map(|line| (line.chars().count() / content_width).max(1))
    .sum::<usize>();
  let height = (line_count as u16 + 2).clamp(3, 8);
  let x = area.x + area.width.saturating_sub(width + 1);
  let y = area.y + 1;

  Rect::new(x, y, width, height.min(area.height.saturating_sub(2)))
}

fn notification_lines(message: &str, area: Rect) -> Vec<Line<'static>> {
  let content_width = area.width.saturating_sub(2).max(1) as usize;
  let max_lines = area.height.saturating_sub(2).max(1) as usize;
  let mut lines = Vec::new();
  let mut truncated = false;

  'outer: for raw_line in message.lines() {
    let chars = raw_line.chars().collect::<Vec<_>>();

    for chunk in chars.chunks(content_width) {
      if lines.len() >= max_lines {
        truncated = true;
        break 'outer;
      }

      lines.push(Line::from(chunk.iter().collect::<String>()));
    }

    if chars.is_empty() && lines.len() < max_lines {
      lines.push(Line::from(String::new()));
    }
  }

  if truncated && let Some(last) = lines.last_mut() {
    let mut text = last.to_string();
    text.truncate(content_width.saturating_sub(3));
    text.push_str("...");
    *last = Line::from(text);
  }

  lines
}

fn selected_device<'a>(devices: &'a [UsbDevice], state: &TableState) -> Option<&'a UsbDevice> {
  state.selected().and_then(|index| devices.get(index))
}

fn select_next(state: &mut TableState, len: usize) {
  if len == 0 {
    state.select(None);
    return;
  }

  let next = state.selected().map_or(0, |index| (index + 1) % len);
  state.select(Some(next));
}

fn select_previous(state: &mut TableState, len: usize) {
  if len == 0 {
    state.select(None);
    return;
  }

  let previous = state
    .selected()
    .map_or(0, |index| if index == 0 { len - 1 } else { index - 1 });
  state.select(Some(previous));
}
