pub mod book_library;
pub mod download_utils;
pub mod telegram_files;
pub mod downloader;

use tracing::log;

use crate::{prisma::cached_file, views::Database};

use self::{download_utils::DownloadResult, telegram_files::{download_from_telegram_files, UploadData, upload_to_telegram_files}, downloader::{get_filename, FilenameData, download_from_downloader}, book_library::{get_book, types::BaseBook, get_books}};


pub async fn get_cached_file_or_cache(
    object_id: i32,
    object_type: String,
    db: Database
) -> Option<cached_file::Data> {
    let cached_file = db.cached_file()
        .find_unique(cached_file::object_id_object_type(object_id, object_type.clone()))
        .exec()
        .await
        .unwrap();

    match cached_file {
        Some(cached_file) => Some(cached_file),
        None => cache_file(object_id, object_type, db).await,
    }
}


pub async fn cache_file(
    object_id: i32,
    object_type: String,
    db: Database
) -> Option<cached_file::Data> {
    let book = match get_book(object_id).await {
        Ok(v) => v,
        Err(err) => {
            log::error!("{:?}", err);
            return None;
        },
    };

    let downloader_result = match download_from_downloader(
        book.source.id,
        object_id,
        object_type.clone()
    ).await {
        Ok(v) => v,
        Err(err) => {
            log::error!("{:?}", err);
            return None;
        },
    };

    let UploadData { chat_id, message_id } = match upload_to_telegram_files(
        downloader_result,
        book.get_caption()
    ).await {
        Ok(v) => v,
        Err(err) => {
            log::error!("{:?}", err);
            return None;
        },
    };

    Some(
        db
        .cached_file()
        .create(
            object_id,
            object_type,
            message_id,
            chat_id,
            vec![]
        )
        .exec()
        .await
        .unwrap()
    )
}


pub async fn download_from_cache(
    cached_data: cached_file::Data,
    db: Database
) -> Option<DownloadResult> {
    let response_task = tokio::task::spawn(download_from_telegram_files(cached_data.message_id, cached_data.chat_id));
    let filename_task = tokio::task::spawn(get_filename(cached_data.object_id, cached_data.object_type.clone()));
    let book_task = tokio::task::spawn(get_book(cached_data.object_id));

    let response = match response_task.await.unwrap() {
        Ok(v) => v,
        Err(err) => {
            db.cached_file()
                .delete(cached_file::object_id_object_type(cached_data.object_id, cached_data.object_type.clone()))
                .exec()
                .await
                .unwrap();

            log::error!("{:?}", err);
            return None;
        },
    };

    let filename_data = match filename_task.await.unwrap() {
        Ok(v) => v,
        Err(err) => {
            log::error!("{:?}", err);
            return None;
        }
    };

    let book = match book_task.await.unwrap() {
        Ok(v) => v,
        Err(err) => {
            log::error!("{:?}", err);
            return None;
        }
    };

    let FilenameData {filename, filename_ascii} = filename_data;
    let caption = book.get_caption();

    Some(DownloadResult {
        response,
        filename,
        filename_ascii,
        caption
    })
}

pub async fn get_books_for_update() -> Result<Vec<BaseBook>, Box<dyn std::error::Error + Send + Sync>> {
    let mut result: Vec<BaseBook> = vec![];

    let page_size = 50;

    let uploaded_gte = "".to_string();
    let uploaded_lte = "".to_string();

    let first_page = match get_books(
        1,
        page_size,
        uploaded_gte.clone(),
        uploaded_lte.clone()
    ).await {
        Ok(v) => v,
        Err(err) => return Err(err),
    };

    result.extend(first_page.items);

    let mut current_page = 2;
    let page_count = first_page.pages;

    while current_page <= page_count {
        let page = match get_books(current_page, page_size, uploaded_gte.clone(), uploaded_lte.clone()).await {
            Ok(v) => v,
            Err(err) => return Err(err),
        };
        result.extend(page.items);

        current_page += 1;
    };

    Ok(result)
}


pub async fn start_update_cache(
    db: Database
) {
    let books = match get_books_for_update().await {
        Ok(v) => v,
        Err(err) => {
            log::error!("{:?}", err);
            return;
        },
    };

    for book in books {
        for available_type in book.available_types {
            let cached_file = match db
                .cached_file()
                .find_unique(
                    cached_file::object_id_object_type(book.id, available_type.clone())
                )
                .exec()
                .await {
                    Ok(v) => v,
                    Err(err) => {
                        log::error!("{:?}", err);
                        continue;
                    }
                };

            if cached_file.is_some() {
                continue;
            }

            cache_file(book.id, available_type, db.clone()).await;
        }
    }
}
