use std::collections::BTreeMap;
use std::sync::Arc;

const BURIKO_MOVIE_MAGIC: &[u8; 16] = b"BF_Movie_______\0";

#[derive(Debug, Clone)]
pub(crate) struct BurikoMovieResource {
    pub archive: String,
    pub resource: String,
    pub bytes: Arc<Vec<u8>>,
    pub metadata: [i32; 5],
    pub parent: Option<i32>,
    pub child: Option<i32>,
}

#[derive(Debug, Default)]
pub(crate) struct BurikoMovieRegistry {
    next_handle: i32,
    resources: BTreeMap<i32, BurikoMovieResource>,
}

impl BurikoMovieRegistry {
    pub(crate) fn load(
        &mut self,
        archive: String,
        resource: String,
        bytes: Vec<u8>,
    ) -> Result<(i32, [i32; 5]), i32> {
        let metadata = parse_buriko_movie_header(&bytes).ok_or(2)?;
        self.next_handle = self.next_handle.saturating_add(1).max(1);
        let handle = self.next_handle;
        self.resources.insert(
            handle,
            BurikoMovieResource {
                archive,
                resource,
                bytes: Arc::new(bytes),
                metadata,
                parent: None,
                child: None,
            },
        );
        Ok((handle, metadata))
    }

    pub(crate) fn contains(&self, handle: i32) -> bool {
        self.resources.contains_key(&handle)
    }

    pub(crate) fn metadata(&self, handle: i32) -> Option<[i32; 5]> {
        self.resources
            .get(&handle)
            .map(|resource| resource.metadata)
    }

    pub(crate) fn release(&mut self, handle: i32) -> i32 {
        let Some(resource) = self.resources.remove(&handle) else {
            return 3;
        };
        if let Some(parent) = resource.parent {
            if let Some(parent_resource) = self.resources.get_mut(&parent) {
                if parent_resource.child == Some(handle) {
                    parent_resource.child = resource.child;
                }
            }
        }
        if let Some(child) = resource.child {
            if let Some(child_resource) = self.resources.get_mut(&child) {
                child_resource.parent = resource.parent;
            }
        }
        0
    }

    pub(crate) fn attach(&mut self, source: i32) -> Result<i32, i32> {
        let Some(source_resource) = self.resources.get(&source) else {
            return Err(3);
        };
        if source_resource.child.is_some() {
            return Err(7);
        }
        let cloned = source_resource.clone();
        self.next_handle = self.next_handle.saturating_add(1).max(1);
        let handle = self.next_handle;
        self.resources.insert(
            handle,
            BurikoMovieResource {
                archive: cloned.archive,
                resource: cloned.resource,
                bytes: cloned.bytes,
                metadata: cloned.metadata,
                parent: Some(source),
                child: None,
            },
        );
        self.resources
            .get_mut(&source)
            .expect("source resource disappeared")
            .child = Some(handle);
        Ok(handle)
    }

    pub(crate) fn validate_decode(
        &self,
        destination_info: Option<(u32, u32, i32)>,
        source: i32,
        frame_index: i32,
    ) -> i32 {
        let Some(resource) = self.resources.get(&source) else {
            return 3;
        };
        let frame_count = resource.metadata[4].max(0);
        if frame_index < 0 || frame_index >= frame_count {
            return 4;
        }
        let Some((width, height, format)) = destination_info else {
            return 5;
        };
        if width as i32 != resource.metadata[0]
            || height as i32 != resource.metadata[1]
            || format != resource.metadata[2]
        {
            return 5;
        }
        // The target reaches status 6 only when its proprietary BMV frame
        // decoder fails after all handle/header/bitmap checks. The portable
        // backend currently validates the complete public contract but cannot
        // decode that codec yet.
        6
    }
}

fn parse_buriko_movie_header(bytes: &[u8]) -> Option<[i32; 5]> {
    if bytes.len() < 44 || &bytes[..16] != BURIKO_MOVIE_MAGIC {
        return None;
    }
    let read = |offset: usize| {
        i32::from_le_bytes(
            bytes[offset..offset + 4]
                .try_into()
                .expect("checked header range"),
        )
    };
    Some([read(20), read(24), read(32), read(36), read(40)])
}

#[cfg(test)]
mod tests {
    use super::{BURIKO_MOVIE_MAGIC, BurikoMovieRegistry, parse_buriko_movie_header};

    fn movie_bytes() -> Vec<u8> {
        let mut bytes = vec![0u8; 64];
        bytes[..16].copy_from_slice(BURIKO_MOVIE_MAGIC);
        for (offset, value) in [(20, 640_i32), (24, 480), (32, 4), (36, 30), (40, 120)] {
            bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
        }
        bytes
    }

    #[test]
    fn parses_the_five_target_metadata_dwords() {
        assert_eq!(
            parse_buriko_movie_header(&movie_bytes()),
            Some([640, 480, 4, 30, 120])
        );
    }

    #[test]
    fn attach_is_single_child_and_release_repairs_links() {
        let mut registry = BurikoMovieRegistry::default();
        let (root, _) = registry
            .load(String::new(), "movie.bmv".into(), movie_bytes())
            .unwrap();
        let child = registry.attach(root).unwrap();
        assert_eq!(registry.attach(root), Err(7));
        assert_eq!(registry.release(child), 0);
        assert!(registry.attach(root).is_ok());
    }
}
