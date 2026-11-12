use clap::Parser;

#[derive(Parser)]
struct Args {
    #[arg(long)]
    name: String,
}

fn main() {
    let args = Args::parse();
    println!("{}", args.name);
}
