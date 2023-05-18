mod evaluate;
mod parse;
mod prepare;
mod object;
mod run;
mod types;

use std::env;
use std::fs;
use std::process::ExitCode;
use std::time::Instant;

use crate::parse::{parse, ParseResult};
use crate::prepare::{prepare, RunNode};
use crate::run::{Frame, RunResult};
use crate::object::Object;

fn main() -> ExitCode {
    let args: Vec<String> = env::args().collect();
    let file_path = if args.len() > 1 {
        &args[1]
    } else {
        "monty.py"
    };
    let code = match read_file(file_path) {
        Ok(code) => code,
        Err(err) => {
            eprintln!("{err}");
            return ExitCode::FAILURE;
        }
    };
    // let input_names = vec!["foo", "bar"];
    // let inputs = vec![Object::Int(1), Object::Int(2)];
    let input_names = vec![];
    let inputs = vec![];

    let ex = Executor::new(&code, file_path, &input_names).unwrap();

    let tic = Instant::now();
    match ex.run(inputs) {
        Ok(_) => {
            let toc = Instant::now();
            eprintln!("Elapsed time: {:?}", toc - tic);
            ExitCode::SUCCESS
        },
        Err(err) => {
            eprintln!("Error running code: {err}");
            ExitCode::FAILURE
        }
    }
}

fn read_file(file_path: &str) -> Result<String, String> {
    eprintln!("Reading file: {file_path}");
    match fs::metadata(file_path) {
        Ok(metadata) => {
            if !metadata.is_file() {
                return Err(format!("Error: {file_path} is not a file"));
            }
        }
        Err(err) => {
            return Err(format!("Error reading {file_path}: {err}"));
        }
    }
    match fs::read_to_string(file_path) {
        Ok(contents) => Ok(contents),
        Err(err) => Err(format!("Error reading file: {err}"))
    }
}

struct Executor {
    initial_namespace: Vec<Object>,
    nodes: Vec<RunNode>,
}

impl Executor {
    fn new(code: &str, filename: &str, input_names: &[&str]) -> ParseResult<Self> {
        let nodes = parse(code, filename)?;
        // dbg!(&nodes);
        let (initial_namespace, nodes) = prepare(nodes, input_names)?;
        dbg!(&initial_namespace, &nodes);
        Ok(Self {
            initial_namespace,
            nodes,
        })
    }

    fn run(&self, inputs: Vec<Object>) -> RunResult<()> {
        let mut namespace = self.initial_namespace.clone();
        for (i, input) in inputs.into_iter().enumerate() {
            namespace[i] = input;
        }
        Frame::new(namespace).execute(&self.nodes)
    }
}
