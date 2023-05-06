from base64 import b64encode

from fastapi import APIRouter, Depends, HTTPException, Request, status
from fastapi.responses import StreamingResponse

from starlette.background import BackgroundTask

from arq.connections import ArqRedis

from app.depends import check_token
from app.models import CachedFile as CachedFileDB
from app.serializers import CachedFile, CreateCachedFile
from app.services.cache_updater import cache_file_by_book_id
from app.services.caption_getter import get_caption
from app.services.downloader import get_filename
from app.services.files_client import download_file as download_file_from_cache
from app.services.library_client import get_book
from app.utils import get_cached_file_or_cache


router = APIRouter(
    prefix="/api/v1", tags=["files"], dependencies=[Depends(check_token)]
)


@router.get("/{object_id}/{object_type}", response_model=CachedFile)
async def get_cached_file(request: Request, object_id: int, object_type: str):
    cached_file = await CachedFileDB.objects.get_or_none(
        object_id=object_id, object_type=object_type
    )

    if not cached_file:
        cached_file = await cache_file_by_book_id(
            {"redis": request.app.state.redis_client}, object_id, object_type
        )

    if not cached_file:
        raise HTTPException(status_code=status.HTTP_404_NOT_FOUND)

    return cached_file


@router.get("/download/{object_id}/{object_type}")
async def download_cached_file(request: Request, object_id: int, object_type: str):
    cached_file = await get_cached_file_or_cache(request, object_id, object_type)
    cache_data: dict = cached_file.data  # type: ignore

    data = await download_file_from_cache(
        cache_data["chat_id"], cache_data["message_id"]
    )
    if data is None:
        await CachedFileDB.objects.filter(id=cached_file.id).delete()

        cached_file = await get_cached_file_or_cache(request, object_id, object_type)
        cache_data: dict = cached_file.data  # type: ignore

        data = await download_file_from_cache(
            cache_data["chat_id"], cache_data["message_id"]
        )

    if data is None:
        raise HTTPException(status_code=status.HTTP_204_NO_CONTENT)

    if (filename := await get_filename(object_id, object_type)) is None:
        raise HTTPException(status_code=status.HTTP_204_NO_CONTENT)

    if (book := await get_book(object_id)) is None:
        raise HTTPException(status_code=status.HTTP_204_NO_CONTENT)

    response, client = data

    async def close():
        await response.aclose()
        await client.aclose()

    filename_ascii = filename.encode("ascii", "ignore").decode("ascii")

    return StreamingResponse(
        response.aiter_bytes(),
        headers={
            "Content-Disposition": f"attachment; filename={filename_ascii}",
            "X-Caption-B64": b64encode(get_caption(book).encode("utf-8")).decode(),
            "X-Filename-B64": b64encode(filename.encode("utf-8")).decode(),
        },
        background=BackgroundTask(close),
    )


@router.delete("/{object_id}/{object_type}", response_model=CachedFile)
async def delete_cached_file(object_id: int, object_type: str):
    cached_file = await CachedFileDB.objects.get_or_none(
        object_id=object_id, object_type=object_type
    )

    if not cached_file:
        raise HTTPException(status_code=status.HTTP_404_NOT_FOUND)

    await cached_file.delete()

    return cached_file


@router.post("/", response_model=CachedFile)
async def create_or_update_cached_file(data: CreateCachedFile):
    cached_file = await CachedFileDB.objects.get_or_none(
        object_id=data.data["object_id"], object_type=data.data["object_type"]
    )

    if cached_file is not None:
        cached_file.message_id = data.data["message_id"]
        cached_file.chat_id = data.data["chat_id"]
        return await cached_file.update()

    return await CachedFileDB.objects.create(
        object_id=data.object_id,
        object_type=data.object_type,
        message_id=data.data["message_id"],
        chat_id=data.data["chat_id"],
    )


@router.post("/update_cache")
async def update_cache(request: Request):
    arq_pool: ArqRedis = request.app.state.arq_pool

    await arq_pool.enqueue_job("check_books")

    return "Ok!"


healthcheck_router = APIRouter(
    tags=["healthcheck"],
)


@healthcheck_router.get("/healthcheck")
async def healthcheck():
    return "Ok!"
