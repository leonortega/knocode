use criterion::{black_box, criterion_group, criterion_main, Criterion};
use tree_sitter_language_pack::{ProcessConfig, process};

const RUST_SAMPLE: &str = r#"
use std::collections::HashMap;

pub struct Config {
    pub name: String,
    pub debug: bool,
}

impl Config {
    pub fn new(name: &str) -> Self {
        Self { name: name.to_string(), debug: false }
    }
}

pub fn process_data(input: &[u8]) -> Result<Vec<u8>, Error> {
    let mut output = Vec::with_capacity(input.len());
    for byte in input {
        output.push(byte ^ 0xFF);
    }
    Ok(output)
}

enum Color { Red, Green, Blue }

trait Drawable { fn draw(&self); }

type Result<T> = std::result::Result<T, Error>;
"#;

const PYTHON_SAMPLE: &str = r#"
import os
from typing import List, Optional

class DataProcessor:
    def __init__(self, config: dict):
        self.config = config
        self.results: List[str] = []

    def process(self, data: str) -> Optional[str]:
        if not data:
            return None
        result = data.strip().lower()
        self.results.append(result)
        return result

    def flush(self) -> List[str]:
        out = self.results.copy()
        self.results.clear()
        return out

def helper_fn(x: int, y: int = 10) -> int:
    return x + y

class Error(Exception):
    pass
"#;

const JS_SAMPLE: &str = r#"
import { EventEmitter } from 'events';

export class ConfigManager extends EventEmitter {
    constructor(defaults = {}) {
        super();
        this.config = { ...defaults };
        this.changed = false;
    }

    get(key) {
        return this.config[key];
    }

    set(key, value) {
        this.config[key] = value;
        this.changed = true;
        this.emit('change', { key, value });
    }
}

export function processData(items) {
    return items
        .filter(item => item.active)
        .map(item => ({ ...item, processed: true }));
}

const helper = () => {
    return { timestamp: Date.now() };
};
"#;

const GO_SAMPLE: &str = r#"
package main

import (
    "fmt"
    "sync"
)

type Server struct {
    addr    string
    port    int
    mu      sync.RWMutex
    running bool
}

func NewServer(addr string, port int) *Server {
    return &Server{addr: addr, port: port}
}

func (s *Server) Start() error {
    s.mu.Lock()
    defer s.mu.Unlock()
    s.running = true
    fmt.Printf("Server starting on %s:%d\n", s.addr, s.port)
    return nil
}

func (s *Server) Stop() {
    s.mu.Lock()
    defer s.mu.Unlock()
    s.running = false
}

type Config struct {
    Debug   bool
    Timeout int
}
"#;

const CSHARP_SAMPLE: &str = r#"
using System;
using System.Collections.Generic;

namespace MyApp.Services
{
    public interface IDataService
    {
        Task<string> GetDataAsync(int id);
        void ProcessData(IEnumerable<string> items);
    }

    public class DataService : IDataService
    {
        private readonly Dictionary<int, string> _cache;

        public DataService()
        {
            _cache = new Dictionary<int, string>();
        }

        public async Task<string> GetDataAsync(int id)
        {
            if (_cache.TryGetValue(id, out var cached))
                return cached;

            var data = await FetchFromApi(id);
            _cache[id] = data;
            return data;
        }

        public void ProcessData(IEnumerable<string> items)
        {
            foreach (var item in items)
            {
                Console.WriteLine($"Processing: {item}");
            }
        }

        private Task<string> FetchFromApi(int id) => Task.FromResult($"data-{id}");
    }

    public record Config(string Name, bool Debug);
}
"#;

const TS_SAMPLE: &str = r#"
interface Config {
    name: string;
    debug: boolean;
    timeout: number;
}

type Result<T> = { ok: true; data: T } | { ok: false; error: string };

class EventBus<T extends Record<string, unknown>> {
    private listeners: Map<string, Array<(data: T) => void>> = new Map();

    on<K extends keyof T>(event: K, handler: (data: T[K]) => void): void {
        const list = this.listeners.get(event as string) || [];
        list.push(handler as (data: T) => void);
        this.listeners.set(event as string, list);
    }

    emit<K extends keyof T>(event: K, data: T[K]): void {
        const list = this.listeners.get(event as string) || [];
        list.forEach(fn => fn(data));
    }
}

function processData<T>(items: T[], predicate: (item: T) => boolean): T[] {
    return items.filter(predicate);
}

export { Config, Result, EventBus, processData };
"#;

fn bench_parse_all_languages(c: &mut Criterion) {
    let samples: Vec<(&str, &str)> = vec![
        ("rust", RUST_SAMPLE),
        ("python", PYTHON_SAMPLE),
        ("javascript", JS_SAMPLE),
        ("go", GO_SAMPLE),
        ("csharp", CSHARP_SAMPLE),
        ("typescript", TS_SAMPLE),
    ];

    // Warm up: ensure all parsers are downloaded and cached
    for &(lang, code) in &samples {
        let config = ProcessConfig::new(lang).all();
        let _ = process(black_box(code), &config);
    }

    // Benchmark per-language parse + symbol extraction
    let mut group = c.benchmark_group("tree_sitter_parse");
    for &(lang, code) in &samples {
        group.bench_function(lang, |b| {
            b.iter(|| {
                let config = ProcessConfig::new(lang).all();
                let result = process(black_box(code), &config).unwrap();
                black_box(&result.structure);
            })
        });
    }
    group.finish();

    // Benchmark symbol count per language
    let mut symbol_group = c.benchmark_group("tree_sitter_symbols");
    for &(lang, code) in &samples {
        let config = ProcessConfig::new(lang).all();
        let result = process(code, &config).unwrap();
        symbol_group.bench_function(lang, |b| {
            b.iter(|| {
                let result = process(black_box(code), &config).unwrap();
                black_box(result.structure.len());
            })
        });
    }
    symbol_group.finish();
}

criterion_group!(benches, bench_parse_all_languages);
criterion_main!(benches);
