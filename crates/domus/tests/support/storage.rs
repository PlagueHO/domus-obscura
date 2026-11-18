use domus::domain::LibraryPath;
use domus::infrastructure::filesystem::{StorageConfiguration, StorageRole, StorageRoot};
use std::path::Path;

pub fn select_host_roots(
    host: &Path,
    additional: &[(&str, StorageRole, &Path)],
) -> Result<(), Box<dyn std::error::Error>> {
    let mut value: serde_json::Value = serde_json::from_slice(&std::fs::read(host)?)?;
    let mut storage: StorageConfiguration = serde_json::from_value(value["storage"].clone())?;
    for provider in &mut storage.providers {
        if provider.boundary.is_relative() {
            provider.boundary = host
                .parent()
                .ok_or("host directory")?
                .join(&provider.boundary);
        }
    }
    let mut roots = storage
        .roots
        .iter()
        .map(|root| {
            (
                root.root_id.clone(),
                root.role,
                storage.providers[0]
                    .boundary
                    .join(root.relative.replace('/', std::path::MAIN_SEPARATOR_STR)),
            )
        })
        .collect::<Vec<_>>();
    roots.extend(
        additional
            .iter()
            .map(|(id, role, path)| ((*id).to_owned(), *role, path.to_path_buf())),
    );
    let boundary = roots[0]
        .2
        .ancestors()
        .find(|boundary| roots.iter().all(|(_, _, path)| path.starts_with(boundary)))
        .ok_or("fixture roots have no common boundary")?;
    storage.providers[0].boundary = boundary.to_path_buf();
    storage.roots = roots
        .iter()
        .map(|(id, role, path)| {
            let relative = path
                .strip_prefix(boundary)?
                .iter()
                .map(|part| part.to_str().ok_or("fixture path is not Unicode"))
                .collect::<Result<Vec<_>, _>>()?
                .join("/");
            Ok(StorageRoot::inspect(
                id,
                1,
                *role,
                "application",
                boundary,
                &LibraryPath::parse(&relative)?,
            )?)
        })
        .collect::<Result<_, Box<dyn std::error::Error>>>()?;
    value["storage"] = serde_json::to_value(storage)?;
    std::fs::write(host, serde_json::to_vec(&value)?)?;
    Ok(())
}
