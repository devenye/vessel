use anyhow::{self, Context, Result};
use colored::*;
use flate2::read::GzDecoder;
use log::debug;
use reqwest;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::process::Command;
use tar::Archive;
use tempfile::TempDir;

pub struct Vessel {
    pub output_for_humans: bool,
    pub package_set: PackageSet,
    pub manifest: Manifest,
}

impl Vessel {
    pub fn new(
        output_for_humans: bool,
        package_set_file: &PathBuf,
        manifest_file: &PathBuf,
    ) -> Result<Vessel> {
        let package_set_file = File::open(package_set_file).context(format!(
            "Failed to open the package set file at {}",
            package_set_file.display()
        ))?;
        let package_set: PackageSet = serde_json::from_reader(package_set_file)
            .context("Failed to parse the package set file")?;
        let manifest_file =
            File::open(manifest_file).context("Failed to open the vessel.json file")?;
        let manifest: Manifest = serde_json::from_reader(manifest_file)
            .context("Failed to parse the vessel.json file")?;
        Ok(Vessel {
            output_for_humans,
            package_set,
            manifest,
        })
    }

    pub fn for_humans<F>(&self, s: F)
    where
        F: FnOnce(),
    {
        if self.output_for_humans {
            s()
        }
    }

    pub fn install_packages(&self) -> Result<Vec<(String, PathBuf)>> {
        let install_plan = self
            .package_set
            .transitive_deps(self.manifest.dependencies.clone());

        self.for_humans(|| {
            println!(
                "{} Installing {} packages",
                "[Info]".blue(),
                install_plan.len()
            )
        });
        for package in &install_plan {
            self.download_package(package)?
        }
        self.for_humans(|| println!("{} Installation complete.", "[Info]".blue()));

        Ok(install_plan
            .iter()
            .map(|p| {
                (
                    p.name.clone(),
                    PathBuf::from(&format!(".vessel/{}/{}/src", p.name, p.version)),
                )
            })
            .collect())
    }

    pub fn download_package(&self, package: &Package) -> Result<()> {
        let package_dir = format!(".vessel/{}", package.name);
        let package_dir = Path::new(&package_dir);
        if !package_dir.exists() {
            fs::create_dir_all(package_dir).context(format!(
                "Failed to create the package directory at {}",
                package_dir.display()
            ))?;
        }
        let repo_dir = package_dir.join(&package.version);
        if !repo_dir.exists() {
            if package.repo.starts_with("https://github.com") {
                self.for_humans(|| {
                    println!(
                        "{} Downloading tar-ball: \"{}\"",
                        "[Info]".blue(),
                        package.name
                    )
                });
                download_tar_ball(&repo_dir, &package.repo, &package.version).or_else(|_| {
                    self.for_humans(|| {
                        println!(
                            "{} Downloading tar-ball failed, cloning as git repo instead: \"{}\"",
                            "[Warn]".yellow(),
                            package.name
                        )
                    });
                    clone_package(&repo_dir, &package.repo, &package.version)
                })?
            } else {
                self.for_humans(|| {
                    println!(
                        "{} Cloning git repository: \"{}\"",
                        "[Info]".blue(),
                        package.name
                    )
                });
                clone_package(&repo_dir, &package.repo, &package.version)?
            }
        } else {
            debug!(
                "{} at version {} has already been downloaded",
                package.name, package.version
            )
        }
        Ok(())
    }
}

/// Downloads and unpacks a tar-ball from Github into the `dest` path
fn download_tar_ball(dest: &Path, repo: &str, version: &str) -> Result<()> {
    let target = format!(
        "{}/archive/{}/.tar.gz",
        repo.trim_end_matches(".git"),
        version
    );
    let response = reqwest::blocking::get(&target)?;

    // We unpack into a temporary directory and rename it in one go once
    // the full unpacking was successful
    let tmp_dir: TempDir = tempfile::tempdir()?;
    Archive::new(GzDecoder::new(response)).unpack(tmp_dir.path())?;

    // We expect an unpacked repo to contain exactly one directory
    let repo_dir = match fs::read_dir(tmp_dir.path())?.next() {
        None => return Err(anyhow::anyhow!("Unpacked an empty tarball for {}", repo)),
        Some(dir) => dir?,
    };

    if !repo_dir.path().is_dir() {
        return Err(anyhow::anyhow!("Failed to unpack tarball for \"{}\"", repo));
    }
    fs::rename(repo_dir.path(), dest)?;

    Ok(())
}

fn clone_package(dest: &Path, repo: &str, version: &str) -> Result<()> {
    let tmp_dir: TempDir = tempfile::tempdir()?;
    Command::new("git")
        .args(&["clone", repo, "repo"])
        .current_dir(tmp_dir.path())
        .output()
        .context(format!("Failed to clone the repo at {}", repo))?;
    let repo_dir = tmp_dir.path().join("repo");
    Command::new("git")
        .args(&["-c", "advice.detachedHead=false", "checkout", version])
        .current_dir(&repo_dir)
        .output()
        .context(format!(
            "Failed to checkout version {} for the repository {} in {}",
            version,
            repo,
            repo_dir.display()
        ))?;
    fs::rename(repo_dir, dest)?;
    Ok(())
}

/// Initializes a new vessel project by creating a `vessel.json` file with no
/// dependencies and adding a dummy package set (for now, we should pull this
/// from a community maintained repository instead)
pub fn init() -> Result<()> {
    let package_set_path: PathBuf = PathBuf::from("package-set.json");
    let manifest_path: PathBuf = PathBuf::from("vessel.json");
    if package_set_path.exists() {
        return Err(anyhow::anyhow!(
            "Failed to initialize, there is an existing package-set.json file here"
        ));
    }
    if manifest_path.exists() {
        return Err(anyhow::anyhow!(
            "Failed to initialize, there is an existing vessel.json file here"
        ));
    }
    let initial_package_set: PackageSet = PackageSet(vec![{
        Package {
            name: "leftpad".to_string(),
            repo: "https://github.com/kritzcreek/mo-leftpad.git".to_string(),
            version: "v1.0.0".to_string(),
            dependencies: vec![],
        }
    }]);
    let initial_manifest: Manifest = Manifest {
        dependencies: vec![],
    };
    let mut package_set_file =
        File::create(package_set_path).context("Failed to create the package-set.json file")?;
    serde_json::to_writer_pretty(&mut package_set_file, &initial_package_set)
        .context("Failed to create the package-set.json file")?;

    let mut manifest_file =
        File::create(manifest_path).context("Failed to create the vessel.json file")?;
    serde_json::to_writer_pretty(&mut manifest_file, &initial_manifest)
        .context("Failed to create the vessel.json file")?;

    Ok(())
}

pub type Url = String;
pub type Tag = String;
pub type Name = String;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Package {
    pub name: Name,
    pub repo: Url,
    pub version: Tag,
    pub dependencies: Vec<Name>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PackageSet(pub Vec<Package>);

#[derive(Debug, PartialEq, Serialize, Deserialize)]
pub struct Manifest {
    pub dependencies: Vec<Name>,
}

impl PackageSet {
    fn find(&self, name: &str) -> &Package {
        self.0
            .iter()
            .find(|p| p.name == *name)
            .unwrap_or_else(|| panic!("Package \"{}\" wasn't specified in the package set", name))
    }

    fn transitive_deps(&self, entry_points: Vec<Name>) -> Vec<&Package> {
        let mut found: HashSet<Name> = HashSet::new();
        let mut todo: Vec<Name> = entry_points;
        while let Some(next) = todo.pop() {
            if !found.contains(&next) {
                todo.append(&mut self.find(&next).dependencies.clone());
                found.insert(next);
            }
        }
        // Once we have incremental compilation we could return these toposorted to allow
        // starting to compile the first packages while others are still being downloaded.
        // For now we sort them to get deterministic behaviour for testing.
        let mut found: Vec<Name> = found.into_iter().collect();
        found.sort();
        found.iter().map(|n| self.find(n)).collect()
    }
}

#[cfg(test)]
mod test {
    use super::*;

    fn mk_package(name: &str, deps: Vec<&str>) -> Package {
        Package {
            name: name.to_string(),
            repo: "".to_string(),
            version: "".to_string(),
            dependencies: deps.into_iter().map(|x| x.to_string()).collect(),
        }
    }

    #[test]
    fn it_finds_a_transitive_dependency() {
        let a = mk_package("A", vec!["B"]);
        let b = mk_package("B", vec![]);
        let ps = PackageSet(vec![a.clone(), b.clone()]);
        assert_eq!(vec![&b], ps.transitive_deps(vec!["B".to_string()]));
        assert_eq!(vec![&a, &b], ps.transitive_deps(vec!["A".to_string()]))
    }

    #[test]
    fn it_finds_transitive_dependencies_with_overlaps() {
        let a = mk_package("A", vec!["B"]);
        let b = mk_package("B", vec![]);
        let c = mk_package("C", vec!["B"]);
        let ps = PackageSet(vec![a.clone(), b.clone(), c.clone()]);
        assert_eq!(
            vec![&a, &b, &c],
            ps.transitive_deps(vec!["A".to_string(), "C".to_string()])
        );

        assert_eq!(vec![&b, &c], ps.transitive_deps(vec!["C".to_string()]))
    }
}
