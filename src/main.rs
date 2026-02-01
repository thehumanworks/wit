use wit::grep;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let repos = grep::client::GrepClient::new().find_repos("ratat", Some("rus")).await.unwrap();
    
    println!("Found {} repos", repos.len());
    for (i,repo) in repos.iter().enumerate() {
        println!("{}. {}", i+1, repo);
    }
    Ok(())
}
