// Embedded dependency: hprof
// Source: https://github.com/rust-lang/rust-analyzer/blob/master/crates/rust-analyzer/src/tracing/hprof.rs
// License: MIT OR Apache-2.0

// Modified version of hprof that includes custom writer.

// Consumer of `tracing` data, which prints a hierarchical profile.
//
// Based on https://github.com/davidbarsky/tracing-tree, but does less, while
// actually printing timings for spans by default. The code here is vendored from
// https://github.com/matklad/tracing-span-tree.
//
// Usage:
//
// ```rust
// let layer = hprof::SpanTree::default();
// Registry::default().with(layer).init();
// ```
//
// Example output:
//
// ```text
// 8.37ms           top_level
//   1.09ms           middle
//     1.06ms           leaf
//   1.06ms           middle
//   3.12ms           middle
//     1.06ms           leaf
//   3.06ms           middle
// ```
//
// Same data, but with `.aggregate(true)`:
//
// ```text
// 8.39ms           top_level
//  8.35ms    4      middle
//    2.13ms    2      leaf
// ```

use std::fmt::Write;
use std::mem;
use std::time::Duration;
use std::time::Instant;

use rustc_hash::FxHashSet;
use tracing::field::Field;
use tracing::field::Visit;
use tracing::span::Attributes;
use tracing::Event;
use tracing::Id;
use tracing::Level;
use tracing::Subscriber;
use tracing_subscriber::filter;
use tracing_subscriber::fmt::MakeWriter;
use tracing_subscriber::layer::Context;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::registry::LookupSpan;
use tracing_subscriber::Layer;
use tracing_subscriber::Registry;

use crate::logger_hprof;

pub fn init(spec: &str) -> tracing::subscriber::DefaultGuard {
    let subscriber = Registry::default().with(layer(spec, std::io::stderr));
    tracing::subscriber::set_default(subscriber)
}

pub fn layer<W, S>(spec: &str, make_writer: W) -> impl Layer<S>
where
    S: Subscriber + for<'span> tracing_subscriber::registry::LookupSpan<'span>,
    W: for<'writer> MakeWriter<'writer> + 'static + Send + Sync,
{
    let (write_filter, allowed_names) = WriteFilter::from_spec(spec);

    // this filter the first pass for `tracing`: these are all the "profiling" spans, but things like
    // span depth or duration are not filtered here: that only occurs at write time.
    let profile_filter = filter::filter_fn(move |metadata| {
        let allowed = match &allowed_names {
            Some(names) => names.contains(metadata.name()),
            None => true,
        };

        allowed &&
            metadata.is_span() &&
            metadata.level() >= &Level::INFO &&
            !metadata.target().starts_with("salsa") &&
            metadata.name() != "compute_exhaustiveness_and_usefulness" &&
            !metadata.target().starts_with("chalk")
    });

    logger_hprof::SpanTree {
        aggregate: false,
        write_filter,
        make_writer,
    }
    .with_filter(profile_filter)
}

#[derive(Debug)]
pub(crate) struct SpanTree<W = fn() -> std::io::Stderr> {
    aggregate: bool,
    write_filter: WriteFilter,
    make_writer: W,
}

struct Data {
    start: Instant,
    children: Vec<Node>,
    fields: String,
}

impl Data {
    fn new(attrs: &Attributes<'_>) -> Self {
        let mut data = Self {
            start: Instant::now(),
            children: Vec::new(),
            fields: String::new(),
        };

        let mut visitor = DataVisitor {
            string: &mut data.fields,
        };
        attrs.record(&mut visitor);
        data
    }

    fn into_node(self, name: &'static str) -> Node {
        Node {
            name,
            fields: self.fields,
            count: 1,
            duration: self.start.elapsed(),
            children: self.children,
        }
    }
}

pub struct DataVisitor<'a> {
    string: &'a mut String,
}

impl<'a> Visit for DataVisitor<'a> {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        write!(self.string, "{} = {:?} ", field.name(), value).unwrap();
    }
}

impl<S, W> Layer<S> for SpanTree<W>
where
    S: Subscriber + for<'span> LookupSpan<'span>,
    W: for<'writer> MakeWriter<'writer> + 'static,
{
    fn on_new_span(&self, attrs: &Attributes<'_>, id: &Id, ctx: Context<'_, S>) {
        let span = ctx.span(id).unwrap();

        let data = Data::new(attrs);
        span.extensions_mut().insert(data);
    }

    fn on_event(&self, _event: &Event<'_>, _ctx: Context<'_, S>) {}

    fn on_close(&self, id: Id, ctx: Context<'_, S>) {
        let span = ctx.span(&id).unwrap();
        let data = span.extensions_mut().remove::<Data>().unwrap();
        let mut node = data.into_node(span.name());

        match span.parent() {
            Some(parent_span) => {
                parent_span
                    .extensions_mut()
                    .get_mut::<Data>()
                    .unwrap()
                    .children
                    .push(node);
            },
            None => {
                if self.aggregate {
                    node.aggregate()
                }
                let mut writer = self.make_writer.make_writer();
                node.print(&self.write_filter, &mut writer)
            },
        }
    }
}

#[derive(Default)]
struct Node {
    name: &'static str,
    fields: String,
    count: u32,
    duration: Duration,
    children: Vec<Node>,
}

impl Node {
    fn print<W>(&self, filter: &WriteFilter, writer: &mut W)
    where
        W: std::io::Write,
    {
        self.go(0, filter, writer)
    }

    #[allow(clippy::print_stderr)]
    fn go<W>(&self, level: usize, filter: &WriteFilter, out: &mut W)
    where
        W: std::io::Write,
    {
        if self.duration > filter.longer_than && level < filter.depth {
            let duration = ms(self.duration);
            let current_indent = level * 2;

            let _ = write!(out, "{:current_indent$}   {duration} {:<6}", "", self.name);

            if !self.fields.is_empty() {
                let _ = write!(out, " @ {}", self.fields);
            }

            if self.count > 1 {
                let _ = write!(out, " ({} calls)", self.count);
            }

            let _ = write!(out, "\n");

            for child in &self.children {
                child.go(level + 1, filter, out)
            }
        }
    }

    fn aggregate(&mut self) {
        if self.children.is_empty() {
            return;
        }

        self.children.sort_by_key(|it| it.name);
        let mut idx = 0;
        for i in 1..self.children.len() {
            if self.children[idx].name == self.children[i].name {
                let child = mem::take(&mut self.children[i]);
                self.children[idx].duration += child.duration;
                self.children[idx].count += child.count;
                self.children[idx].children.extend(child.children);
            } else {
                idx += 1;
                assert!(idx <= i);
                self.children.swap(idx, i);
            }
        }
        self.children.truncate(idx + 1);
        for child in &mut self.children {
            child.aggregate()
        }
    }
}

#[derive(Default, Clone, Debug)]
pub(crate) struct WriteFilter {
    depth: usize,
    longer_than: Duration,
}

impl WriteFilter {
    pub(crate) fn from_spec(mut spec: &str) -> (WriteFilter, Option<FxHashSet<String>>) {
        let longer_than = if let Some(idx) = spec.rfind('>') {
            let longer_than = spec[idx + 1..]
                .parse()
                .expect("invalid profile longer_than");
            spec = &spec[..idx];
            Duration::from_millis(longer_than)
        } else {
            Duration::new(0, 0)
        };

        let depth = if let Some(idx) = spec.rfind('@') {
            let depth: usize = spec[idx + 1..].parse().expect("invalid profile depth");
            spec = &spec[..idx];
            depth
        } else {
            999
        };
        let allowed = if spec == "*" {
            None
        } else {
            Some(FxHashSet::from_iter(spec.split('|').map(String::from)))
        };
        (WriteFilter { depth, longer_than }, allowed)
    }
}

#[allow(non_camel_case_types)]
struct ms(Duration);

impl std::fmt::Display for ms {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let n = self.0.as_millis();
        write!(f, "{n:5}ms")
    }
}