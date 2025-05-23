use serde::ser::{Serialize, SerializeMap, Serializer};
use std::borrow::BorrowMut;
use std::fmt::{self, Display, Formatter};
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::Path;
use std::str::FromStr;
use std::{collections::HashMap, future::Future, path::PathBuf, pin::Pin};
use url::Url;

pub type FileAccessorResult<T> = Pin<Box<dyn Future<Output = Result<T, String>>>>;

pub trait FileAccessor {
    fn file_exists(&self, path: String) -> FileAccessorResult<bool>;
    fn read_file(&self, path: String) -> FileAccessorResult<String>;
    fn read_contracts_content(
        &self,
        contracts_paths: Vec<String>,
    ) -> FileAccessorResult<HashMap<String, String>>;
    fn write_file(&self, path: String, content: &[u8]) -> FileAccessorResult<()>;
}

#[derive(Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(untagged)]
pub enum FileLocation {
    FileSystem { path: PathBuf },
    Url { url: Url },
}

impl Hash for FileLocation {
    fn hash<H: Hasher>(&self, state: &mut H) {
        match self {
            Self::FileSystem { path } => {
                let canonicalized_path = path.canonicalize().unwrap_or(path.clone());
                canonicalized_path.hash(state);
            }
            Self::Url { url } => {
                url.hash(state);
            }
        }
    }
}

impl Display for FileLocation {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            FileLocation::FileSystem { path } => write!(f, "{}", path.display()),
            FileLocation::Url { url } => write!(f, "{}", url),
        }
    }
}

impl FileLocation {
    pub fn try_parse(
        location_string: &str,
        workspace_root_location_hint: Option<&FileLocation>,
    ) -> Option<FileLocation> {
        if let Ok(location) = FileLocation::from_url_string(location_string) {
            return Some(location);
        }
        if let Ok(FileLocation::FileSystem { path }) =
            FileLocation::from_path_string(location_string)
        {
            match (workspace_root_location_hint, path.is_relative()) {
                (None, true) => return None,
                (Some(hint), true) => {
                    let mut location = hint.clone();
                    location.append_path(location_string).ok()?;
                    return Some(location);
                }
                (_, false) => return Some(FileLocation::FileSystem { path }),
            }
        }
        None
    }

    pub fn from_path(path: PathBuf) -> FileLocation {
        FileLocation::FileSystem { path }
    }

    pub fn from_url(url: Url) -> FileLocation {
        FileLocation::Url { url }
    }

    pub fn from_url_string(url_string: &str) -> Result<FileLocation, String> {
        let url = Url::from_str(url_string)
            .map_err(|e| format!("unable to parse {} as a url\n{:?}", url_string, e))?;

        #[cfg(not(feature = "wasm"))]
        if url.scheme() == "file" {
            let path =
                url.to_file_path().map_err(|_| format!("unable to conver url {} to path", url))?;
            return Ok(FileLocation::FileSystem { path });
        }

        Ok(FileLocation::Url { url })
    }

    pub fn working_dir() -> FileLocation {
        FileLocation::from_path_string(".").unwrap()
    }

    pub fn from_path_string(path_string: &str) -> Result<FileLocation, String> {
        let path = PathBuf::from_str(path_string)
            .map_err(|e| format!("unable to parse {} as a path\n{:?}", path_string, e))?;
        Ok(FileLocation::FileSystem { path })
    }

    pub fn append_path(&mut self, path_string: &str) -> Result<(), String> {
        let path_to_append = PathBuf::from_str(path_string)
            .map_err(|e| format!("unable to read relative path {}\n{:?}", path_string, e))?;
        match self.borrow_mut() {
            FileLocation::FileSystem { path } => {
                path.extend(&path_to_append);
            }
            FileLocation::Url { url } => {
                let mut paths_segments =
                    url.path_segments_mut().map_err(|_| "unable to mutate url".to_string())?;
                for component in path_to_append.components() {
                    let segment = component
                        .as_os_str()
                        .to_str()
                        .ok_or(format!("unable to format component {:?}", component))?;
                    paths_segments.push(segment);
                }
            }
        }
        Ok(())
    }

    pub fn expect_path_buf(&self) -> PathBuf {
        match self {
            FileLocation::FileSystem { path } => path.clone(),
            FileLocation::Url { .. } => unreachable!(),
        }
    }

    pub fn read_content_as_utf8(&self) -> Result<String, String> {
        let content = self.read_content()?;
        let contract_as_utf8 = String::from_utf8(content).map_err(|e| {
            format!("unable to read content from {} as utf8 ({})", self, e.to_string())
        })?;
        Ok(contract_as_utf8)
    }

    fn fs_read_content(path: &Path) -> Result<Vec<u8>, String> {
        use std::fs::File;
        use std::io::{BufReader, Read};
        let file = File::open(path)
            .map_err(|e| format!("unable to read file {} ({})", path.display(), e.to_string()))?;
        let mut file_reader = BufReader::new(file);
        let mut file_buffer = vec![];
        file_reader
            .read_to_end(&mut file_buffer)
            .map_err(|e| format!("unable to read file {} ({})", path.display(), e.to_string()))?;
        Ok(file_buffer)
    }

    fn fs_exists(path: &Path) -> bool {
        path.exists()
    }

    fn fs_write_content(file_path: &PathBuf, content: &[u8]) -> Result<(), String> {
        use std::fs::{self, File};
        use std::io::Write;
        let mut parent_directory = file_path.clone();
        parent_directory.pop();
        fs::create_dir_all(&parent_directory).map_err(|e| {
            format!("unable to create parent directory {}\n{}", parent_directory.display(), e)
        })?;
        let mut file = File::create(file_path)
            .map_err(|e| format!("unable to open file {}\n{}", file_path.display(), e))?;
        file.write_all(content)
            .map_err(|e| format!("unable to write file {}\n{}", file_path.display(), e))?;
        Ok(())
    }

    pub fn get_workspace_root_location(&self) -> Result<FileLocation, String> {
        let mut workspace_root_location = self.clone();
        match workspace_root_location.borrow_mut() {
            FileLocation::FileSystem { path } => {
                let mut manifest_found = false;
                while path.pop() {
                    path.push("txtx.yml");
                    if FileLocation::fs_exists(path) {
                        path.pop();
                        manifest_found = true;
                        break;
                    }
                    path.pop();
                }

                match manifest_found {
                    true => Ok(workspace_root_location),
                    false => Err(format!("unable to find root location from {}", self)),
                }
            }
            _ => {
                unimplemented!();
            }
        }
    }

    pub async fn get_workspace_manifest_location(
        &self,
        file_accessor: Option<&dyn FileAccessor>,
    ) -> Result<FileLocation, String> {
        match file_accessor {
            None => {
                let mut project_root_location = self.get_workspace_root_location()?;
                project_root_location.append_path("txtx.yml")?;
                Ok(project_root_location)
            }
            Some(file_accessor) => {
                let mut manifest_location = None;
                let mut parent_location = self.get_parent_location();
                while let Ok(ref parent) = parent_location {
                    let mut candidate = parent.clone();
                    candidate.append_path("txtx.yml")?;

                    if let Ok(exists) = file_accessor.file_exists(candidate.to_string()).await {
                        if exists {
                            manifest_location = Some(candidate);
                            break;
                        }
                    }
                    if &parent.get_parent_location().unwrap() == parent {
                        break;
                    }
                    parent_location = parent.get_parent_location();
                }
                match manifest_location {
                    Some(manifest_location) => Ok(manifest_location),
                    None => Err(format!(
                        "No Clarinet.toml is associated to the contract {}",
                        self.get_file_name().unwrap_or_default()
                    )),
                }
            }
        }
    }

    pub fn get_absolute_path(&self) -> Result<PathBuf, String> {
        match self {
            FileLocation::FileSystem { path } => {
                let abs = fs::canonicalize(path)
                    .map_err(|e| format!("failed to get absolute path: {e}"))?;
                Ok(abs)
            }
            FileLocation::Url { url } => {
                return Err(format!("cannot get absolute path for url {}", url))
            }
        }
    }

    pub fn get_parent_location(&self) -> Result<FileLocation, String> {
        let mut parent_location = self.clone();
        match &mut parent_location {
            FileLocation::FileSystem { path } => {
                let mut parent = path.clone();
                parent.pop();
                if parent.to_str() == path.to_str() {
                    return Err(String::from("reached root"));
                }
                path.pop();
            }
            FileLocation::Url { url } => {
                let mut segments =
                    url.path_segments_mut().map_err(|_| "unable to mutate url".to_string())?;
                segments.pop();
            }
        }
        Ok(parent_location)
    }

    pub fn get_relative_path_from_base(
        &self,
        base_location: &FileLocation,
    ) -> Result<String, String> {
        let file = self.to_string();
        Ok(file[(base_location.to_string().len() + 1)..].to_string())
    }

    pub fn get_relative_location(&self) -> Result<String, String> {
        let base = self.get_workspace_root_location().map(|l| l.to_string())?;
        let file = self.to_string();
        let offset = if base.is_empty() { 0 } else { 1 };
        Ok(file[(base.len() + offset)..].to_string())
    }

    pub fn get_file_name(&self) -> Option<String> {
        match self {
            FileLocation::FileSystem { path } => {
                path.file_name().and_then(|f| Some(f.to_str()?.to_string()))
            }
            FileLocation::Url { url } => {
                url.path_segments().and_then(|p| Some(p.last()?.to_string()))
            }
        }
    }
}

impl FileLocation {
    pub fn read_content(&self) -> Result<Vec<u8>, String> {
        let bytes = match &self {
            FileLocation::FileSystem { path } => FileLocation::fs_read_content(path),
            FileLocation::Url { url } => match url.scheme() {
                #[cfg(not(feature = "wasm"))]
                "file" => {
                    let path = url
                        .to_file_path()
                        .map_err(|e| format!("unable to convert url {} to path\n{:?}", url, e))?;
                    FileLocation::fs_read_content(&path)
                }
                "http" | "https" => {
                    unimplemented!()
                }
                _ => {
                    unimplemented!()
                }
            },
        }?;
        Ok(bytes)
    }

    pub fn exists(&self) -> bool {
        match self {
            FileLocation::FileSystem { path } => FileLocation::fs_exists(path),
            FileLocation::Url { url: _url } => unimplemented!(),
        }
    }

    pub fn write_content(&self, content: &[u8]) -> Result<(), String> {
        match self {
            FileLocation::FileSystem { path } => FileLocation::fs_write_content(path, content),
            FileLocation::Url { url: _url } => unimplemented!(),
        }
    }

    pub fn to_url_string(&self) -> Result<String, String> {
        match self {
            #[cfg(not(feature = "wasm"))]
            FileLocation::FileSystem { path } => {
                let file_path = self.to_string();
                let url = Url::from_file_path(file_path)
                    .map_err(|_| format!("unable to conver path {} to url", path.display()))?;
                Ok(url.to_string())
            }
            FileLocation::Url { url } => Ok(url.to_string()),
            #[allow(unreachable_patterns)]
            _ => unreachable!(),
        }
    }
}

impl Serialize for FileLocation {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut map = serializer.serialize_map(Some(1))?;
        match self {
            FileLocation::FileSystem { path: _ } => {
                let path = match self.get_relative_location() {
                    Ok(relative_path) => relative_path, // Use relative path if possible
                    Err(_) => self.to_string(),         // Fallback on fully qualified path
                };
                map.serialize_entry("path", &path)?;
            }
            FileLocation::Url { url } => {
                map.serialize_entry("url", &url.to_string())?;
            }
        }
        map.end()
    }
}

pub fn get_manifest_location(path: Option<String>) -> Option<FileLocation> {
    if let Some(path) = path {
        let manifest_path = PathBuf::from(path);
        if !manifest_path.exists() {
            return None;
        }
        Some(FileLocation::from_path(manifest_path))
    } else {
        let mut current_dir = std::env::current_dir().unwrap();
        loop {
            current_dir.push("txtx.yml");

            if current_dir.exists() {
                return Some(FileLocation::from_path(current_dir));
            }
            current_dir.pop();

            if !current_dir.pop() {
                return None;
            }
        }
    }
}

pub fn get_txtx_files_paths(
    dir: &str,
    environment_selector: &Option<String>,
) -> Result<Vec<PathBuf>, Box<dyn std::error::Error>> {
    let dir = std::fs::read_dir(dir)?;
    let mut files_paths = vec![];
    for res in dir.into_iter() {
        let Ok(dir_entry) = res else {
            continue;
        };
        let path = dir_entry.path();

        // if our path is a file with an extension
        if let Some(ext) = path.extension() {
            // and that extension is either "tx" or "txvars"
            if ["tx", "txvars"].contains(&ext.to_str().unwrap()) {
                // and it has a filename (always true if we have an extension)
                if let Some(file_name) = path.file_name() {
                    let comps = file_name.to_str().unwrap().split(".").collect::<Vec<_>>();
                    // if it has more than two components
                    if comps.len() > 2 {
                        // then we require that the second component match the environment (i.e. signers.devnet.tx)
                        let Some(env) = environment_selector else {
                            continue;
                        };
                        if comps[comps.len() - 2].eq(env) {
                            files_paths.push(path);
                        }
                    // it it doesn't have more than two components, include the file
                    } else {
                        files_paths.push(path);
                    }
                }
            }
        }
        // otherwise, if the path is a directory
        else if path.is_dir() {
            let component = path.components().last().expect("dir has no components");
            if let Some(folder) = component.as_os_str().to_str() {
                // and that directory's top folder matches the env (such as devnet/signers.tx)
                if let Some(env) = environment_selector {
                    if folder.eq(env) {
                        // then we recurse into that directory
                        let mut sub_files_paths = get_txtx_files_paths(
                            &path.to_str().expect("couldn't turn path back to string"),
                            environment_selector,
                        )?;
                        files_paths.append(&mut sub_files_paths);
                    }
                }
            }
        }
    }

    Ok(files_paths)
}

pub fn get_path_from_components(comps: Vec<&str>) -> String {
    let mut path = PathBuf::new();
    for comp in comps {
        path.push(comp);
    }
    path.display().to_string()
}
