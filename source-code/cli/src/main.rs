use miette::{IntoDiagnostic, Result};
use hammer_core::Logger;
use lexopt::{Arg, Parser, ValueExt};
use nix::unistd::Uid;
use owo_colors::OwoColorize;
use std::env;
use std::path::PathBuf;
use std::process::{Command, Stdio};

const BIN_DIR: &str = "/usr/lib/HackerOS/hammer/bin";

fn main() -> Result<()> {
    Logger::init()?;

    let args: Vec<String> = env::args().collect();
    let mut parser = Parser::from_env();

    // Peek at the first argument to decide dispatch
    let arg = parser.next().into_diagnostic()?;
    
    match arg {
        Some(Arg::Value(val)) => {
            let command = val.string().into_diagnostic()?;
            match command.as_str() {
                // CONTAINER APPS
                "install" => run_binary("hammer-containers", &["install"], &args[2..])?,
                "remove-app" => run_binary("hammer-containers", &["remove"], &args[2..])?,
                "list-apps" => run_binary("hammer-containers", &["list"], &args[2..])?,

                // SYSTEM UPDATES
                "update" => require_root(|| run_binary("hammer-updater", &["update"], &args[2..]))?,
                "layer" => require_root(|| run_binary("hammer-updater", &["layer"], &args[2..]))?,
                "clean" => require_root(|| run_binary("hammer-updater", &["clean"], &args[2..]))?,
                "rollback" => require_root(|| run_binary("hammer-updater", &["rollback"], &args[2..]))?,
                
                // UTILS
                "read-only" | "ro" => require_root(|| run_binary("hammer-read", &[], &args[2..]))?,
                
                "help" => print_help(),
                "version" => print_version(),
                _ => {
                     print_help();
                     println!("\n{}", format!("   ERROR: Unknown command '{}'", command).black().on_red());
                     std::process::exit(1);
                }
            }
        }
        Some(Arg::Long("help")) | Some(Arg::Short('h')) => print_help(),
        Some(Arg::Long("version")) | Some(Arg::Short('v')) => print_version(),
        None => print_help(),
        _ => return Ok(()),
    }

    Ok(())
}

fn require_root<F>(f: F) -> Result<()> 
where F: FnOnce() -> Result<()> 
{
    if !Uid::current().is_root() {
        println!("{}", " ACCESS DENIED: Root privileges required.".red().bold());
        println!(" Run with: {}", "sudo hammer <command>".yellow());
        std::process::exit(1);
    }
    f()
}

fn run_binary(binary_name: &str, prefix_args: &[&str], user_args: &[String]) -> Result<()> {
    let binary_path = PathBuf::from(BIN_DIR).join(binary_name);
    
    let mut final_args: Vec<String> = Vec::new();
    for p in prefix_args {
        final_args.push(p.to_string());
    }
    final_args.extend_from_slice(user_args);

    let cmd_to_run = if binary_path.exists() {
        binary_path.to_string_lossy().to_string()
    } else {
        binary_name.to_string()
    };

    let mut child = Command::new(cmd_to_run)
        .args(&final_args)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()
        .into_diagnostic()?;

    let status = child.wait().into_diagnostic()?;

    if !status.success() {
        std::process::exit(status.code().unwrap_or(1));
    }

    Ok(())
}

fn print_help() {
    println!("{}", r#"
                                             +=======                                               
                                           +++**+#######%%                                          
                                          ++*#####**##%%%%%                                         
                           ##           ++*##+*#*+#%%%#%%%@%@                                       
                           ####       ++*#**###++#%%%%@@@@@%%                  **#%%@               
                    ###+    ####      +++======*%%%%%@@@@@@%%           ##%####%%##%@@              
                     ##+=    ####    =++++++++=*%%%@@@@@@@%%@        **######%##%%%@@@              
                        ###  ######  +*%%%%%%%*#%@@@@@@@@@%@     ######%##%%%%%@@@@@@@              
                         ###% %##%   +*%%%%%%#*#%@@@@@%@@@%%%###%%%%#%##%%%%%@@@@@%%@               
                    ***    ########  -+#%%%%%#*#%@@@@@@@@@%%%%#####%%%%%@@@@%%%%%%                  
               **#   ####   ####**## -+%%##%%#*#%@%@@@@@@@%%@%%%%%%%@@@@@@%@   ##                   
                 ###    #### ###***# -+%%##%%#*#%@@@@@@@@@%%@@@@@@@@@@%%%    ####                   
                   ####  #### ###***#=+%%##%@#*#%@@@@@@@@@%%  @@@@%%    #####                       
             ##*+   %####  #####**++*=+%%##%@#+#%%@@@@@@@@%%%%%%%%    ####**   **                   
              ##*###  ########*##*+++=+#%%%%@#+#%@%%%%%@@%%%%%%   **##**#%   ##**                   
                 #*####%*****+++**+==-+###%%%#+*%#%%%%@@%%**  ## %##*++*##%####                     
                   ########+++**=+*+=-=**#%%%*=+#%%%%%%%%#++ #####****#######   ####                
              **#### ##**#*#*+=+*++*=--+**###*-+#%@%%%%%#*++#####*++##**#######%                    
               %#########*+++**+=====--=++***+-+##%%%##%*==#**#**###+*########   ###                
                 %#########*+=====---:::===+*=:=+**%%**+=--***++##++##*#####   #####                
              ##    ###******+=-===------=====:-=+++*+==-:-=--==+++*#####%#######                   
              ######  ###*======-----:::::---:::-=++===::--:-==-+***###########                     
                  ######***++=----:::::::::::::::----::::::----=+++++++*#######%###%                
                     #%###**++++++=====------:::::-------=+++++++***#######%#%                      
             %%%%%%@@@@@@@@%%%%%%%%%%#####***++**+***##%%%%%%%%%%%%%%%%%%%%%%%%%@@@@                
             %%#====+%+===+%@#++++*%%%*=++#@%@@*=+%+==+%%@@@+=*%%*======+%*======+%%@@              
             %@@%-:+%@@-:=%%+-------@@+----*@%=--=%@*--:#@#---+@%+------=%+--------%%%              
              %@%--*@@@=-+%*=-=%@*-=%@+:-==--:---+%@*-----:---+@%=-+@@@#*%+-=@@@*-=%%%              
              @@%--====--+%*=-+%@*=+%@+:=%%*-=#*==%@*-+#=-=#=-+@@+-===+#@%=-=@@@*-=%%%              
              @@%=--=---=+%*=-----=+%@*-=%@#**%*==%@*-+%**#@=-*@@+-=**+*@%=------=*%%%              
              @@%==*%%%+=*%*+=+%%*++@@#++%@@@@@#++%@#=+@@@@@+=*@@*=+@@%%@%+=+#=-*%@@                
             @%@%==*@@@++*%*+=*@@#++@%#++#@@@@%*+*%@#=+%@@@%*+*%@*+*%%%#*%*=+%*==+%%%@              
             %@@%+=#@@@*+*%####@@%##@%####%@@@%####%%###%@%#####@%#######%*+*@@+=+*%%%              
            @%@%++=#@@@*++*%@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@*++*@@@#+++%%%@            
           @%%%%%%%%@@@%%%%@@%@ @@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@ %%@@%%%%@@@@%%%%%%%@           
            @%@@@@@@@%@@@@@@@@  @@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@  %@@@@@@@@@@@@@@@%%            
"#.cyan().bold());
   
   println!("   {}", "NEXT-GEN SYSTEM MANAGER".black().on_magenta());
   println!("   Atomic Updates - Btrfs Snapshots - Isolated Apps\n");

    let print_cmd = |cmd: &str, desc: &str| {
        println!("   {: <20} {}", cmd.green().bold(), desc.bright_black());
    };

    println!("{}", " APPLICATIONS".yellow().bold());
    print_cmd("install <pkg>", "Install CLI/GUI app in container");
    print_cmd("remove-app <pkg>", "Remove installed app wrapper");
    print_cmd("list-apps", "List all containerized apps");

    println!("\n{}", " SYSTEM & UPDATES".blue().bold());
    print_cmd("update", "Atomic system update (Snapshot -> Update)");
    print_cmd("layer <pkg>", "Install package on host via snapshot");
    print_cmd("rollback", "Revert system to previous state");
    print_cmd("clean", "Prune old snapshots");

    println!("\n{}", " SECURITY".red().bold());
    print_cmd("read-only", "Manage file system locks");
    
    println!();
}

fn print_version() {
    println!("hammer 1.1.0 (Btrfs @layout edition)");
}